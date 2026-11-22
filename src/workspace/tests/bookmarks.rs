use std::{
    fs::{self, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{symlink, PermissionsExt},
    },
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use worminal_workspace::bookmarks::{parse_server, Bookmark, Bookmarks, Target, MAX_BOOKMARKS};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../build/checks")
            .join(format!(
                "bookmarks-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn path(&self) -> PathBuf {
        self.0.join("bookmarks.yaml")
    }
    fn store(&self) -> Bookmarks {
        Bookmarks::new(self.path())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn terminal(directory: &str, master: Option<&str>) -> Bookmark {
    Bookmark {
        target: Target::Terminal {
            directory: directory.into(),
            master: master.map(Into::into),
        },
        title: None,
    }
}
fn host(endpoint: &str) -> Bookmark {
    host_on(endpoint, None)
}
fn host_on(endpoint: &str, port: Option<u16>) -> Bookmark {
    Bookmark {
        target: Target::Host {
            endpoint: endpoint.into(),
            port,
        },
        title: None,
    }
}
fn web(url: &str) -> Bookmark {
    Bookmark {
        target: Target::Web { url: url.into() },
        title: Some("Example".into()),
    }
}

#[test]
fn toggling_is_durable_ordered_and_shared_between_owners() {
    let fixture = Fixture::new();
    let first = fixture.store();
    assert!(first.list().unwrap().is_empty());
    assert!(first.toggle(terminal("/srv/app", None)).unwrap());
    assert!(first.toggle(web("https://example.org")).unwrap());
    // The same directory on another host is another bookmark.
    assert!(first
        .toggle(terminal("/srv/app", Some("me@build")))
        .unwrap());
    // A second owner must read current state, not publish a stale in-memory list.
    let second = fixture.store();
    assert!(second.toggle(terminal("/home/me", None)).unwrap());
    let targets: Vec<_> = first
        .list()
        .unwrap()
        .into_iter()
        .map(|b| b.target)
        .collect();
    assert_eq!(
        targets,
        [
            terminal("/srv/app", None).target,
            web("https://example.org").target,
            terminal("/srv/app", Some("me@build")).target,
            terminal("/home/me", None).target,
        ]
    );
    // Identity is the target: a different title still removes the bookmark.
    let mut retitled = web("https://example.org");
    retitled.title = Some("Other".into());
    assert!(!first.toggle(retitled).unwrap());
    assert!(!first.remove(&web("https://example.org").target).unwrap());
    assert!(first.remove(&terminal("/home/me", None).target).unwrap());
    assert_eq!(second.list().unwrap().len(), 2);
    assert_eq!(
        fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn hand_edited_yaml_is_read_and_toggling_keeps_its_entries() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path(),
        "# mine\nbookmarks:\n  - directory: /srv/app\n    master: me@build\n    title: 2024\n  - url: https://example.org/a?b=c\n",
    )
    .unwrap();
    let store = fixture.store();
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].title.as_deref(), Some("2024"));
    assert!(store.toggle(terminal("/tmp/x", None)).unwrap());
    let after = store.list().unwrap();
    assert_eq!(&after[..2], &listed[..]);
    assert_eq!(after.len(), 3);
}

#[test]
fn damaged_files_and_symlinks_are_preserved_instead_of_reset() {
    for damaged in [
        "bookmarks: {a: b}\n",
        "other: []\n",
        "bookmarks:\n  - title: nothing to open\n",
        "bookmarks:\n  - directory: /a\n    url: https://example.org\n",
        "bookmarks:\n  - url: https://example.org\n    master: host\n",
        "bookmarks:\n  - directory: relative/path\n",
        "bookmarks:\n  - directory: /a\n    command: rm -rf /\n",
        "bookmarks:\n  - directory: /a\n  - directory: /a\n",
        "bookmarks:\n  - directory: /a\n    master: -oProxyCommand=x\n",
        "bookmarks:\n  - url: has space\n",
        "bookmarks:\n  - host: -oProxyCommand=x\n",
        "bookmarks:\n  - host: a b\n",
        "bookmarks:\n  - host: me@build\n    master: other\n",
        "bookmarks:\n  - host: me@build\n    directory: /a\n",
        "bookmarks:\n  - host: me@build\n    url: https://example.org\n",
        "bookmarks:\n  - host: me@build\n  - host: me@build\n",
        "bookmarks: [\n",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.path(), damaged).unwrap();
        let store = fixture.store();
        assert!(store.list().is_err(), "{damaged:?} was accepted");
        assert!(store.toggle(terminal("/b", None)).is_err());
        assert_eq!(fs::read_to_string(fixture.path()).unwrap(), damaged);
    }
    let fixture = Fixture::new();
    let target = fixture.0.join("elsewhere.yaml");
    fs::write(&target, "bookmarks: []\n").unwrap();
    symlink(&target, fixture.path()).unwrap();
    assert!(fixture.store().list().is_err());
    assert!(fixture.store().toggle(terminal("/b", None)).is_err());
    assert_eq!(fs::read_to_string(&target).unwrap(), "bookmarks: []\n");
}

#[test]
fn a_busy_file_rejects_before_read_modify_publish() {
    let fixture = Fixture::new();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(fixture.0.join("bookmarks.yaml.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    assert_eq!(
        fixture
            .store()
            .toggle(terminal("/a", None))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!fixture.path().exists());
    drop(lock);
    assert!(fixture.store().toggle(terminal("/a", None)).unwrap());
}

#[test]
fn bounds_and_input_validation_preserve_published_bookmarks() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for n in 0..MAX_BOOKMARKS {
        assert!(store.toggle(terminal(&format!("/d{n}"), None)).unwrap());
    }
    let before = fs::read(fixture.path()).unwrap();
    assert!(store.toggle(terminal("/one-too-many", None)).is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    // Removing still works at the limit.
    assert!(!store.toggle(terminal("/d0", None)).unwrap());

    let fixture = Fixture::new();
    let store = fixture.store();
    for bad in [
        terminal("relative", None),
        terminal("", None),
        terminal("/with\nnewline", None),
        terminal(&format!("/{}", "x".repeat(4096)), None),
        terminal("/ok", Some("-oProxyCommand=x")),
        terminal("/ok", Some("a b")),
        web(""),
        web("has space"),
        web(&format!("https://{}", "x".repeat(4096))),
        Bookmark {
            title: Some("tab\there".into()),
            ..web("https://example.org")
        },
    ] {
        assert!(store.toggle(bad.clone()).is_err(), "{bad:?} was accepted");
    }
    assert!(!fixture.path().exists());
}

#[test]
fn a_full_file_rejects_before_publishing_more_than_it_can_read_back() {
    let fixture = Fixture::new();
    let store = fixture.store();
    // Long paths reach the 64 KiB file limit well before the entry limit.
    let mut added = 0;
    let rejected = loop {
        let long = format!("/{added:04}{}", "x".repeat(4000));
        match store.toggle(terminal(&long, None)) {
            Ok(_) => added += 1,
            Err(error) => break error,
        }
        assert!(added < MAX_BOOKMARKS, "the size limit never applied");
    };
    assert_eq!(rejected.kind(), std::io::ErrorKind::InvalidInput);
    assert!(added > 10);
    // Everything published still reads back whole.
    assert_eq!(store.list().unwrap().len(), added);
}

#[test]
fn a_host_is_an_ssh_endpoint_that_is_added_once_and_read_back_by_hand() {
    let fixture = Fixture::new();
    let store = fixture.store();
    assert!(store.add(host("me@build")).unwrap());
    // Adding what is there is harmless and changes nothing on disk.
    let before = fs::read(fixture.path()).unwrap();
    assert!(!store.add(host("me@build")).unwrap());
    assert!(!store
        .add(Bookmark {
            title: Some("renamed".into()),
            ..host("me@build")
        })
        .unwrap());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    // The same words as a directory or a page are different bookmarks.
    assert!(store.add(terminal("/srv/app", Some("me@build"))).unwrap());
    assert!(store.add(host("[::1]")).unwrap());
    assert!(store.add(host("build-2.example.org")).unwrap());
    assert!(fs::read_to_string(fixture.path())
        .unwrap()
        .contains("host: me@build"));
    let listed = store.list().unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0], host("me@build"));
    // Toggling removes the host like any other bookmark.
    assert!(!store.toggle(host("me@build")).unwrap());
    assert!(store.remove(&host("[::1]").target).unwrap());
    assert_eq!(store.list().unwrap().len(), 2);

    // Hand-written entries are read, with an optional title.
    fs::write(
        fixture.path(),
        "bookmarks:\n  - host: digitize@Luna\n    title: Luna\n  - host: 10.0.0.7\n",
    )
    .unwrap();
    let listed = store.list().unwrap();
    assert_eq!(listed[0].target, host("digitize@Luna").target);
    assert_eq!(listed[0].title.as_deref(), Some("Luna"));
    assert_eq!(listed[1], host("10.0.0.7"));
}

#[test]
fn hosts_that_could_be_options_or_commands_are_rejected_before_publishing() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for bad in [
        "",
        "-oProxyCommand=x",
        "a b",
        "me@",
        "@host",
        "a@b@c",
        "host;reboot",
        "host\n",
        "$(id)",
        "a`id`",
        "user:pw@host",
    ] {
        assert!(store.add(host(bad)).is_err(), "{bad:?} was accepted");
        assert!(store.toggle(host(bad)).is_err(), "{bad:?} was toggled in");
    }
    assert!(store.add(host(&"x".repeat(256))).is_err());
    assert!(!fixture.path().exists());
}

#[test]
fn a_bookmark_is_named_renamed_and_unnamed_by_its_target() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let target = host("me@build").target;
    assert!(store.add(host("me@build")).unwrap());
    assert!(store.add(terminal("/srv/app", None)).unwrap());
    assert!(store.rename(&target, Some("Build box".into())).unwrap());
    assert_eq!(store.list().unwrap()[0].title.as_deref(), Some("Build box"));
    // Same name again changes nothing on disk; a new one replaces it; None clears it.
    let before = fs::read(fixture.path()).unwrap();
    assert!(!store.rename(&target, Some("Build box".into())).unwrap());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    assert!(store.rename(&target, Some("CI".into())).unwrap());
    assert!(fs::read_to_string(fixture.path())
        .unwrap()
        .contains("title: CI"));
    assert!(store.rename(&target, None).unwrap());
    assert_eq!(store.list().unwrap()[0].title, None);
    // Names work on every kind, keep their order, and are refused when they could not be read back.
    assert!(store
        .rename(&terminal("/srv/app", None).target, Some("App".into()))
        .unwrap());
    assert_eq!(store.list().unwrap()[0].target, target);
    assert!(store.rename(&target, Some("a\nb".into())).is_err());
    assert!(store.rename(&target, Some(String::new())).is_err());
    assert!(store.rename(&target, Some("x".repeat(257))).is_err());
    // A bookmark that was removed meanwhile is reported, not recreated.
    assert!(store.remove(&target).unwrap());
    assert_eq!(
        store
            .rename(&target, Some("Gone".into()))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn a_server_port_is_part_of_its_identity_and_survives_the_file() {
    let fixture = Fixture::new();
    let store = fixture.store();
    assert!(store.add(host_on("me@build", Some(2222))).unwrap());
    // The same host on another port, and on ssh's default, are other bookmarks.
    assert!(store.add(host_on("me@build", Some(2200))).unwrap());
    assert!(store.add(host("me@build")).unwrap());
    assert!(!store.add(host_on("me@build", Some(2222))).unwrap());
    let listed: Vec<_> = store
        .list()
        .unwrap()
        .into_iter()
        .map(|b| b.target)
        .collect();
    assert_eq!(
        listed,
        [
            host_on("me@build", Some(2222)).target,
            host_on("me@build", Some(2200)).target,
            host("me@build").target,
        ]
    );
    let text = fs::read_to_string(fixture.path()).unwrap();
    assert!(
        text.contains("port: 2222") && text.contains("port: 2200"),
        "{text}"
    );
    assert!(store
        .remove(&host_on("me@build", Some(2200)).target)
        .unwrap());
    assert_eq!(store.list().unwrap().len(), 2);
}

#[test]
fn a_hand_written_port_is_read_and_port_22_is_the_default() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path(),
        "bookmarks:\n  - host: me@build\n    port: 2222\n  - host: other\n    port: 22\n",
    )
    .unwrap();
    let listed: Vec<_> = fixture
        .store()
        .list()
        .unwrap()
        .into_iter()
        .map(|b| b.target)
        .collect();
    assert_eq!(
        listed,
        [host_on("me@build", Some(2222)).target, host("other").target]
    );
    for bad in [
        "bookmarks:\n  - host: h\n    port: 0\n",
        "bookmarks:\n  - host: h\n    port: 70000\n",
        "bookmarks:\n  - host: h\n    port: ssh\n",
        "bookmarks:\n  - directory: /a\n    port: 22\n",
        "bookmarks:\n  - url: https://example.org\n    port: 22\n",
    ] {
        fs::write(fixture.path(), bad).unwrap();
        assert!(fixture.store().list().is_err(), "{bad:?} was accepted");
    }
    assert!(host_on("h", Some(0)).validate().is_err());
}

#[test]
fn typed_servers_split_the_port_off_without_breaking_ipv6() {
    for (typed, endpoint, port) in [
        ("build", "build", None),
        ("me@build", "me@build", None),
        ("me@build:2222", "me@build", Some(2222)),
        (" build:2222 ", "build", Some(2222)),
        ("build:22", "build", None),
        ("[::1]", "[::1]", None),
        ("[::1]:2222", "[::1]", Some(2222)),
        ("me@[fe80::1]:2200", "me@[fe80::1]", Some(2200)),
        ("::1", "::1", None),
        ("10.0.0.7:65535", "10.0.0.7", Some(65535)),
    ] {
        assert_eq!(
            parse_server(typed).unwrap(),
            (endpoint.to_owned(), port),
            "{typed:?}"
        );
    }
    for bad in [
        "",
        "build:",
        "build:0",
        "build:65536",
        "build:ssh",
        "me@build:22x",
        "-o:2222",
        "a b:22",
        "host;reboot:22",
        "me@:22",
    ] {
        assert!(parse_server(bad).is_err(), "{bad:?} was accepted");
    }
}
