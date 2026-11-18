use std::{
    fs::{self, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{symlink, PermissionsExt},
    },
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use worminal_protocol::worp::devices::{Device, MAX_DEVICES};
use worminal_workspace::devices::Registry;

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../build/checks")
            .join(format!(
                "registry-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("devices.yaml")
    }
    fn registry(&self) -> Registry {
        Registry::new(self.path())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn device(name: &str, endpoint: &str) -> Device {
    Device {
        name: name.into(),
        endpoint: endpoint.into(),
    }
}

#[test]
fn explicit_membership_is_durable_idempotent_and_never_silently_rebound() {
    let fixture = Fixture::new();
    let first = fixture.registry();
    assert!(first.list().unwrap().is_empty());
    assert!(first.enroll(device("cinnamon", "felix@cinnamon")).unwrap());
    assert!(!first.enroll(device("cinnamon", "felix@cinnamon")).unwrap());
    let before = fs::read(fixture.path()).unwrap();
    assert!(first.enroll(device("cinnamon", "another-host")).is_err());
    assert!(first.enroll(device("alias", "felix@cinnamon")).is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    // A second owner must read current state, not publish a stale in-memory registry.
    let second = fixture.registry();
    assert!(second.enroll(device("anvil", "anvil")).unwrap());
    assert_eq!(
        first.list().unwrap(),
        vec![
            device("anvil", "anvil"),
            device("cinnamon", "felix@cinnamon")
        ]
    );
    assert!(first.forget("anvil").unwrap());
    assert!(!first.forget("anvil").unwrap());
    assert_eq!(
        second.list().unwrap(),
        vec![device("cinnamon", "felix@cinnamon")]
    );
    assert_eq!(
        fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn malformed_state_and_symlinks_are_preserved_instead_of_reset() {
    let fixture = Fixture::new();
    for malformed in [
        "unrecognized registry",
        "devices: {a: {endpoint: x}}\nextra: 1\n",
        "devices: {a: {endpoint: x, tags: [gpu]}}\n",
        "devices: {a: {endpoint: x}, b: {endpoint: x}}\n",
        "devices: {a: {endpoint: x}, a: {endpoint: y}}\n",
        "devices: {UPPER: {endpoint: x}}\n",
        "devices: {a: {endpoint: 'ssh://x'}}\n",
    ] {
        fs::write(fixture.path(), malformed).unwrap();
        assert!(fixture.registry().list().is_err(), "{malformed}");
        assert!(fixture.registry().enroll(device("bench", "bench")).is_err());
        assert_eq!(fs::read_to_string(fixture.path()).unwrap(), malformed);
    }
    fs::remove_file(fixture.path()).unwrap();
    let target = fixture.0.join("preserved");
    fs::write(&target, b"preserve me").unwrap();
    symlink(&target, fixture.path()).unwrap();
    assert!(fixture.registry().enroll(device("bench", "bench")).is_err());
    assert!(fixture.registry().forget("bench").is_err());
    assert_eq!(fs::read(target).unwrap(), b"preserve me");
}

#[test]
fn hand_edited_yaml_is_membership_and_enrollment_keeps_it() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path(),
        "# mine\ndevices:\n  zeta: {endpoint: me@zeta}\n  42:\n    endpoint: box42\n",
    )
    .unwrap();
    let registry = fixture.registry();
    assert_eq!(
        registry.list().unwrap(),
        vec![device("42", "box42"), device("zeta", "me@zeta")]
    );
    assert!(registry.enroll(device("anvil", "anvil")).unwrap());
    assert_eq!(
        registry.list().unwrap(),
        vec![
            device("42", "box42"),
            device("anvil", "anvil"),
            device("zeta", "me@zeta")
        ]
    );
    for empty in ["", "devices:\n", "devices: {}\n"] {
        fs::write(fixture.path(), empty).unwrap();
        assert!(registry.list().unwrap().is_empty(), "{empty:?}");
    }
}

#[test]
fn busy_registry_rejects_before_read_modify_publish() {
    let fixture = Fixture::new();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(fixture.0.join("devices.yaml.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    assert_eq!(
        fixture
            .registry()
            .enroll(device("bench", "bench"))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!fixture.path().exists());
    drop(lock);
    assert!(fixture.registry().enroll(device("bench", "bench")).unwrap());
}

#[test]
fn enrollment_bounds_and_input_validation_preserve_published_membership() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    for n in 0..MAX_DEVICES {
        registry
            .enroll(device(&format!("node{n}"), &format!("host{n}")))
            .unwrap();
    }
    let before = fs::read(fixture.path()).unwrap();
    assert!(registry.enroll(device("extra", "extra")).is_err());
    for bad in [
        device("../escape", "host"),
        device("UPPER", "host"),
        device("valid", "ssh://host"),
        device("valid", "user@"),
        device("valid", "a@@b"),
        device("valid", "user:password@host"),
    ] {
        assert!(registry.enroll(bad).is_err());
    }
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
}
