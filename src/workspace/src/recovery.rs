use std::{
    fs::OpenOptions,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use worminal_protocol::{Buffer, Payload, WIRE_VERSION};

// Recovery recipe format. Saved recipes outlive the binaries that wrote them.
const RESTORE_VERSION: u32 = 1;
const RESTORE_LIMIT: u32 = 8 * 1024 * 1024;
const RESTORE_TABS: u32 = 4096;
const RESTORE_URL_LIMIT: u32 = 65536;
const RESTORE_DIRECTORY_LIMIT: usize = libc::PATH_MAX as usize;
/// One-shot marker telling a recovered shell that it replaces a saved terminal.
pub use worminal_protocol::worp::launch::RESTORE_ENV_NAME;

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
        self.directory.len() < RESTORE_DIRECTORY_LIMIT
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

impl Recovery {
    /// The object always survives a read failure, with writes disabled. A caller cannot accidentally
    /// replace damaged or unrecognized recipes with the empty workspace used after recovery fails.
    pub fn open(path: PathBuf) -> (Self, io::Result<Vec<Recipe>>) {
        let read = || -> io::Result<(Vec<u8>, Vec<Recipe>)> {
            crate::statefile::prepare_directory(&path)?;
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
        crate::statefile::atomic_write(&self.path, &bytes)?;
        self.cache = Cache::Valid(bytes);
        Ok(true)
    }
}
