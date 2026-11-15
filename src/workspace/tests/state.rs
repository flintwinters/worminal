use std::{
    fs,
    io::Read,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use worminal_protocol::Buffer;
use worminal_workspace::{
    launch::Launch,
    recovery::{self, Recipe, Recovery},
    rights,
};

#[test]
fn descriptor_bundles_are_owned_cloexec_and_closed_after_marker_or_capacity_rejection() {
    use std::{
        fs::File,
        os::fd::{AsFd, AsRawFd},
        os::unix::net::UnixStream,
    };
    let fixture = Fixture::new();
    let path = fixture.root.join("descriptor-file");
    fs::write(&path, b"bundle-bytes").unwrap();
    let file = File::open(&path).unwrap();
    let copies = || {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| fs::read_link(entry.path()).is_ok_and(|target| target == path))
            .count()
    };
    let (sender, receiver) = UnixStream::pair().unwrap();
    assert_eq!(copies(), 1);
    rights::send_descriptors(sender.as_fd(), &[file.as_fd(), file.as_fd()], b'H').unwrap();
    let received = rights::receive_descriptors(receiver.as_fd(), b'H', 2).unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(copies(), 3);
    for descriptor in &received {
        let info =
            fs::read_to_string(format!("/proc/self/fdinfo/{}", descriptor.as_raw_fd())).unwrap();
        let flags = info
            .lines()
            .find_map(|line| line.strip_prefix("flags:\t"))
            .unwrap();
        assert_ne!(
            u32::from_str_radix(flags, 8).unwrap() & libc::O_CLOEXEC as u32,
            0
        );
    }
    let mut received = received.into_iter();
    let mut bytes = Vec::new();
    File::from(received.next().unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"bundle-bytes");
    drop(received);
    assert_eq!(copies(), 1);
    rights::send_descriptors(sender.as_fd(), &[file.as_fd()], b'H').unwrap();
    assert!(rights::receive_descriptors(receiver.as_fd(), b'X', 1).is_err());
    assert_eq!(copies(), 1);
    rights::send_descriptors(
        sender.as_fd(),
        &[file.as_fd(), file.as_fd(), file.as_fd()],
        b'H',
    )
    .unwrap();
    assert!(rights::receive_descriptors(receiver.as_fd(), b'H', 1).is_err());
    assert_eq!(copies(), 1);
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    scope: String,
}

impl Fixture {
    fn new() -> Self {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let scope = format!(
            "rust-workspace-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let root = project.join("build/checks").join(&scope);
        fs::create_dir_all(&root).unwrap();
        Self { root, scope }
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn path(&self) -> PathBuf {
        recovery::path(&self.state(), self.scope.as_bytes()).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn request(environment: &[&[u8]]) -> Vec<u8> {
    let mut output = Buffer::default();
    for value in [80, 24, 1, 1, environment.len() as u32] {
        output.put_u32(value).unwrap();
    }
    for field in [
        b"title".as_slice(),
        b"/directory",
        b"",
        b"-",
        b"42",
        b"/bin/sh",
    ] {
        output.put_string(field).unwrap();
    }
    for field in environment {
        output.put_string(field).unwrap();
    }
    output.into_bytes()
}

#[test]
fn launch_bounds_environment_and_restore_marker() {
    let inherited = Launch::decode(&request(&[])).unwrap();
    assert!(inherited.environment.is_none());
    let explicit = Launch::decode(&request(&[
        b"KEEP=yes",
        b"WORMINAL_RESTORE=old",
        b"ignored",
    ]))
    .unwrap();
    assert_eq!(explicit.arguments[0].as_bytes(), b"/bin/sh");
    let restored = explicit.environment_for_restore();
    assert_eq!(restored.len(), 2);
    assert!(restored
        .iter()
        .any(|(key, value)| key == "WORMINAL_RESTORE" && value == "1"));
    let mut invalid = request(&[]);
    invalid[..4].copy_from_slice(&401u32.to_be_bytes());
    assert!(Launch::decode(&invalid).is_err());
    assert!(Launch::decode(&request(&[])[..10]).is_err());
    let mut invalid = request(&[]);
    invalid.push(0);
    assert!(Launch::decode(&invalid).is_err());
}

#[test]
fn invalid_recipes_and_symlinks_cannot_be_overwritten() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().parent().unwrap()).unwrap();
    fs::write(fixture.path(), b"damaged recipe").unwrap();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.is_err());
    assert!(!recovery.save(&[Recipe::for_tab(b"/", b"", b"/")]).unwrap());
    assert_eq!(fs::read(fixture.path()).unwrap(), b"damaged recipe");
    fs::remove_file(fixture.path()).unwrap();
    let target = fixture.root.join("preserved");
    fs::write(&target, b"do not follow").unwrap();
    symlink(&target, fixture.path()).unwrap();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.is_err());
    assert!(!recovery.save(&[]).unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"do not follow");
}

#[test]
fn invalid_writes_preserve_the_last_published_recipe() {
    let fixture = Fixture::new();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.unwrap().is_empty());
    recovery.save(&[Recipe::for_tab(b"/", b"", b"/")]).unwrap();
    let before = fs::read(fixture.path()).unwrap();
    assert!(recovery
        .save(&[Recipe {
            directory: b"relative".to_vec(),
            url: Vec::new()
        }])
        .is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    let mut truncated = before.clone();
    truncated.push(0);
    assert!(recovery::decode(&truncated).is_err());
    assert!(!fs::read_dir(fixture.path().parent().unwrap())
        .unwrap()
        .any(|entry| entry.unwrap().file_name().as_bytes().ends_with(b".pending")));
}
