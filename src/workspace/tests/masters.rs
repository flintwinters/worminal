use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use worminal_workspace::masters::{Master, Registry};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let checks = std::env::var_os("WORMINAL_TEST_CHECKS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../build/checks")
            });
        let path = checks.join(format!(
            "masters-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn registry(&self) -> Registry {
        Registry::new(self.0.join("history"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn history_is_durable_and_missing_history_is_read_only() {
    let fixture = Fixture::new();
    let registry = fixture.registry();
    assert!(registry.list().unwrap().is_empty());
    assert!(!fixture.0.join("history").exists());
    let connection = registry.connected("user@host").unwrap();
    assert_eq!(
        registry.list().unwrap(),
        vec![Master {
            endpoint: "user@host".into(),
            connected: true
        }]
    );
    drop(connection);
    assert_eq!(
        fixture.registry().list().unwrap(),
        vec![Master {
            endpoint: "user@host".into(),
            connected: false
        }]
    );
}

#[test]
fn independent_leases_and_simultaneous_inspection_do_not_change_liveness() {
    let fixture = Fixture::new();
    let first = fixture.registry().connected("host").unwrap();
    let second = fixture.registry().connected("host").unwrap();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..100 {
                    assert!(fixture.registry().list().unwrap()[0].connected);
                }
            });
        }
    });
    drop(first);
    assert!(fixture.registry().list().unwrap()[0].connected);
    drop(second);
    assert!(!fixture.registry().list().unwrap()[0].connected);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..100 {
                    assert!(!fixture.registry().list().unwrap()[0].connected);
                }
            });
        }
    });
}

#[test]
fn endpoint_identity_supports_ipv6_aliases_navigation_names_and_maximum_length() {
    let fixture = Fixture::new();
    let maximum = "x".repeat(255);
    let mut endpoints = vec![
        ".".to_owned(),
        "..".to_owned(),
        "alias".to_owned(),
        "u@[::1]:22".to_owned(),
        maximum,
    ];
    let leases: Vec<_> = endpoints
        .iter()
        .map(|endpoint| fixture.registry().connected(endpoint).unwrap())
        .collect();
    endpoints.sort();
    assert_eq!(
        fixture
            .registry()
            .list()
            .unwrap()
            .into_iter()
            .map(|row| row.endpoint)
            .collect::<Vec<_>>(),
        endpoints
    );
    drop(leases);
    assert!(fixture
        .registry()
        .list()
        .unwrap()
        .iter()
        .all(|row| !row.connected));
    for invalid in ["", "../escape", "bad\nhost", "-host"] {
        assert!(fixture.registry().connected(invalid).is_err());
    }
}

#[test]
fn symlinks_and_nonempty_records_are_rejected_without_modification() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");
    fs::write(&outside, "preserve").unwrap();
    let directory = fixture.0.join("history");
    fs::create_dir(&directory).unwrap();
    symlink(&outside, directory.join("host")).unwrap();
    assert!(fixture.registry().connected("host").is_err());
    assert!(fixture.registry().list().is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "preserve");
    fs::remove_file(directory.join("host")).unwrap();
    fs::write(directory.join("host"), "corrupt").unwrap();
    assert!(fixture.registry().connected("host").is_err());
    assert!(fixture.registry().list().is_err());
    assert_eq!(
        fs::read_to_string(directory.join("host")).unwrap(),
        "corrupt"
    );
}
