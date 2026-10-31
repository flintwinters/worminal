use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use worminal_protocol::{Buffer, Payload, WIRE_VERSION};

#[allow(non_camel_case_types, non_upper_case_globals)]
mod format {
    include!(concat!(env!("OUT_DIR"), "/restore.rs"));
}
pub use format::RESTORE_ENV_NAME;
use format::{
    RESTORE_DIRECTORY_LIMIT, RESTORE_LIMIT, RESTORE_TABS, RESTORE_URL_LIMIT, RESTORE_VERSION,
};

#[derive(Debug, PartialEq, Eq)]
pub struct Recipe {
    pub directory: Vec<u8>,
    pub url: Vec<u8>,
}

impl Recipe {
    pub fn for_tab(directory: &[u8], url: &[u8], home: &[u8]) -> Self {
        let candidate = if directory.is_empty() {
            home
        } else {
            directory
        };
        Self {
            directory: if candidate.starts_with(b"/") {
                candidate
            } else {
                b"/"
            }
            .to_vec(),
            url: url.to_vec(),
        }
    }

    fn valid(&self) -> bool {
        self.directory.len() < RESTORE_DIRECTORY_LIMIT as usize
            && self.url.len() <= RESTORE_URL_LIMIT as usize
            && !self.directory.contains(&0)
            && !self.url.contains(&0)
            && (self.directory.is_empty() || self.directory.starts_with(b"/"))
            && !(self.directory.is_empty() && self.url.is_empty())
    }
}

pub fn decode(bytes: &[u8]) -> io::Result<Vec<Recipe>> {
    if bytes.len() > RESTORE_LIMIT as usize {
        return Err(invalid());
    }
    let mut payload = Payload::new(bytes);
    if payload.u32()? != RESTORE_VERSION {
        return Err(invalid());
    }
    let count = payload.u32()?;
    if count > RESTORE_TABS {
        return Err(invalid());
    }
    let mut recipes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let recipe = Recipe {
            directory: payload.string()?.to_vec(),
            url: payload.string()?.to_vec(),
        };
        if !recipe.valid() {
            return Err(invalid());
        }
        recipes.push(recipe);
    }
    payload.finish()?;
    Ok(recipes)
}

fn encode(recipes: &[Recipe]) -> io::Result<Vec<u8>> {
    if recipes.len() > RESTORE_TABS as usize {
        return Err(invalid());
    }
    let mut buffer = Buffer::with_limit(RESTORE_LIMIT as usize);
    buffer.put_u32(RESTORE_VERSION)?;
    buffer.put_u32(recipes.len() as u32)?;
    for recipe in recipes {
        if !recipe.valid() {
            return Err(invalid());
        }
        buffer.put_string(&recipe.directory)?;
        buffer.put_string(&recipe.url)?;
    }
    Ok(buffer.into_bytes())
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid tab recovery file; preserve the original",
    )
}

pub fn path(state_home: &Path, scope: &[u8]) -> io::Result<PathBuf> {
    if !state_home.is_absolute() || scope.len() > 127 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid recovery root or workspace scope",
        ));
    }
    let scope: String = scope.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(state_home
        .join("worminal")
        .join(format!("tabs-v{WIRE_VERSION}-{scope}")))
}

enum Cache {
    Fresh,
    Valid(Vec<u8>),
    Disabled,
}

pub struct Recovery {
    path: PathBuf,
    cache: Cache,
}

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

impl Recovery {
    /// The object always survives a read failure, with writes disabled. A caller cannot accidentally
    /// replace damaged or unrecognized recipes with the empty workspace used after recovery fails.
    pub fn open(path: PathBuf) -> (Self, io::Result<Vec<Recipe>>) {
        let read = || -> io::Result<(Vec<u8>, Vec<Recipe>)> {
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path)?;
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.len() < 8
                || metadata.len() > u64::from(RESTORE_LIMIT)
            {
                return Err(invalid());
            }
            let mut bytes = vec![0; metadata.len() as usize];
            file.read_exact(&mut bytes)?;
            let recipes = decode(&bytes)?;
            Ok((bytes, recipes))
        };
        match read() {
            Ok((bytes, recipes)) => (
                Self {
                    path,
                    cache: Cache::Valid(bytes),
                },
                Ok(recipes),
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (
                Self {
                    path,
                    cache: Cache::Fresh,
                },
                Ok(Vec::new()),
            ),
            Err(error) => (
                Self {
                    path,
                    cache: Cache::Disabled,
                },
                Err(error),
            ),
        }
    }

    pub fn save(&mut self, recipes: &[Recipe]) -> io::Result<bool> {
        if matches!(self.cache, Cache::Disabled)
            || (recipes.is_empty() && matches!(self.cache, Cache::Fresh))
        {
            return Ok(false);
        }
        let bytes = encode(recipes)?;
        if matches!(&self.cache, Cache::Valid(saved) if *saved == bytes) {
            return Ok(false);
        }
        let parent = self.path.parent().ok_or_else(invalid)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let mut file = None;
        for _ in 0..64 {
            let sequence = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
            let mut name = self.path.as_os_str().to_os_string();
            name.push(format!(".{}.{}.pending", std::process::id(), sequence));
            let candidate = PathBuf::from(name);
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&candidate)
            {
                Ok(handle) => {
                    file = Some((PendingFile(candidate), handle));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let (pending, mut file) = file.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "cannot reserve recovery temporary file",
            )
        })?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&pending.0, &self.path)?;
        // Match the existing codec: fsync the containing directory when available after publishing.
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        self.cache = Cache::Valid(bytes);
        Ok(true)
    }
}
