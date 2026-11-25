//! Keep the largest checkpoint from each recent hour in which tabs were removed.
use super::{decode, invalid, Recipe, RESTORE_LIMIT, RESTORE_TABS, RESTORE_VERSION};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const SLOTS: usize = 8;
const SECONDS_PER_HOUR: u64 = 3600;

pub struct History(PathBuf);

#[derive(Clone, Copy)]
pub struct Checkpoint {
    /// Unix timestamp in seconds at which the previous workspace was archived.
    pub id: u64,
    pub tabs: u32,
}

impl History {
    pub(super) fn new(path: &Path) -> Self {
        Self(path.to_owned())
    }

    fn slot(&self, index: usize) -> PathBuf {
        let mut name = self.0.as_os_str().to_owned();
        name.push(format!(".history-{index}"));
        name.into()
    }

    fn read(&self, index: usize) -> io::Result<(Checkpoint, File)> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.slot(index))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || !(16..=8 + u64::from(RESTORE_LIMIT)).contains(&metadata.len()) {
            return Err(invalid());
        }
        let mut header = [0; 16];
        file.read_exact(&mut header)?;
        let id = u64::from_be_bytes(header[..8].try_into().unwrap());
        let version = u32::from_be_bytes(header[8..12].try_into().unwrap());
        let tabs = u32::from_be_bytes(header[12..].try_into().unwrap());
        if version != RESTORE_VERSION || !(1..=RESTORE_TABS).contains(&tabs) {
            return Err(invalid());
        }
        Ok((Checkpoint { id, tabs }, file))
    }

    fn slots(&self) -> io::Result<[Option<Checkpoint>; SLOTS]> {
        let mut slots = [None; SLOTS];
        for (index, entry) in slots.iter_mut().enumerate() {
            match self.read(index) {
                Ok((checkpoint, _)) => *entry = Some(checkpoint),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) if error.kind() == io::ErrorKind::InvalidData => {}
                Err(error) => return Err(error),
            }
        }
        Ok(slots)
    }

    pub fn list(&self) -> io::Result<Vec<Checkpoint>> {
        let mut checkpoints: Vec<_> = self.slots()?.into_iter().flatten().collect();
        checkpoints.sort_unstable_by_key(|checkpoint| std::cmp::Reverse(checkpoint.id));
        Ok(checkpoints)
    }

    pub fn load(&self, id: u64) -> io::Result<Vec<Recipe>> {
        for index in 0..SLOTS {
            // Keep the descriptor that supplied the ID. An atomic slot replacement cannot change it.
            match self.read(index) {
                Ok((checkpoint, file)) if checkpoint.id == id => {
                    let mut bytes = RESTORE_VERSION.to_be_bytes().to_vec();
                    bytes.extend(checkpoint.tabs.to_be_bytes());
                    file.take(u64::from(RESTORE_LIMIT) + 1)
                        .read_to_end(&mut bytes)?;
                    return decode(&bytes);
                }
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Recovery checkpoint not found. Run worminald recover list.",
        ))
    }

    pub(super) fn archive(&self, bytes: &[u8]) -> io::Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs();
        let tabs = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let slots = self.slots()?;
        let current_hour = slots.iter().position(|entry| {
            entry.is_some_and(|entry| entry.id / SECONDS_PER_HOUR == now / SECONDS_PER_HOUR)
        });
        let index = if let Some(index) = current_hour {
            if slots[index].unwrap().tabs >= tabs {
                return Ok(());
            }
            index
        } else {
            slots
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.map(|entry| entry.id))
                .unwrap()
                .0
        };
        // Reserve a distinct ID even if two larger workspaces were lost in the same second.
        let mut id = now;
        while slots.iter().flatten().any(|entry| entry.id == id) {
            id += 1;
        }
        let mut snapshot = id.to_be_bytes().to_vec();
        snapshot.extend_from_slice(bytes);
        crate::statefile::atomic_write(&self.slot(index), &snapshot)
    }
}
