use super::*;
use std::{
    thread,
    time::{Duration, Instant},
};

struct Fixture {
    tab: Tab,
    reaper: ChildReaper,
}
impl Fixture {
    fn new() -> Self {
        Self::running(vec!["/bin/cat".into()])
    }
    fn running(arguments: Vec<OsString>) -> Self {
        let reaper = ChildReaper::default();
        let mut launch = Launch::restore(Vec::new());
        launch.arguments = arguments;
        let tab = Tab::shell(1, &launch, false, Some(7), reaper.clone()).unwrap();
        Self { tab, reaper }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.tab.terminal.as_mut().unwrap().pty.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.reaper.reap() != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(self.reaper.reap(), 0);
    }
}

fn cells(tab: &mut Tab) -> Vec<(u32, u32, u32, u32)> {
    tab.terminal
        .as_mut()
        .unwrap()
        .engine
        .all_rows(1)
        .unwrap()
        .1
        .into_iter()
        .flat_map(|(_, row)| {
            row.into_iter()
                .map(|cell| (cell.rune, cell.attributes, cell.foreground, cell.background))
        })
        .collect()
}

#[test]
fn arbitrary_chunks_keep_utf8_effects_and_terminal_replies_in_order() {
    let bytes = concat!(
        "aé🙂",
        "\x1b]2;renamed\x07",
        "\x1b]4;1;#123456\x07",
        "\x1b]4;1;?\x07",
        "\x1b[1;1H\x1b[6n",
        "\x1b]2;\x07",
        "end"
    )
    .as_bytes();
    let mut whole = Fixture::new();
    whole.tab.feed(bytes, 1).unwrap();
    let expected = cells(&mut whole.tab);
    for chunk in [1, 2, 3, 4, 7, bytes.len()] {
        let mut split = Fixture::new();
        for bytes in bytes.chunks(chunk) {
            split.tab.feed(bytes, 1).unwrap();
        }
        assert_eq!(cells(&mut split.tab), expected);
        assert_eq!(split.tab.title, b"Worminal");
        assert_eq!(split.tab.colors, whole.tab.colors);
        assert!(split.tab.terminal.as_ref().unwrap().pending.is_empty());
        assert_eq!(
            split.tab.terminal.as_ref().unwrap().input.pending(),
            b"\x1b]4;1;rgb:1212/3434/5656\x07\x1b[1;1R"
        );
    }
}

#[test]
fn synchronized_output_holds_catalog_and_reset_releases_palette_storage() {
    let mut fixture = Fixture::new();
    let tab = &mut fixture.tab;
    let before = tab.metadata(1).unwrap().to_vec();
    tab.feed(b"\x1b[?2026h\x1b]2;held\x07\x1b]4;1;#123456\x07", 2)
        .unwrap();
    assert_eq!(tab.metadata(3).unwrap(), before);
    assert!(tab.catalog_pending);
    assert!(tab.frame(3).unwrap().unwrap().redraw_deadline > 3);
    tab.feed(b"\x1b[?2026l", 4).unwrap();
    assert_ne!(tab.metadata(4).unwrap(), before);
    assert!(!tab.catalog_pending);
    tab.feed(b"\x1b]104;1\x07", 5).unwrap();
    assert!(tab.colors.is_none());
}

#[test]
fn native_pending_output_is_consumed_before_new_bytes() {
    let mut fixture = Fixture::new();
    fixture.tab.terminal.as_mut().unwrap().pending = b"legacy\xc3".to_vec();
    fixture.tab.feed(b"\xa9", 1).unwrap();
    let terminal = fixture.tab.terminal.as_mut().unwrap();
    assert!(terminal.pending.is_empty());
    let runes: Vec<_> = terminal.engine.all_rows(1).unwrap().1[0]
        .1
        .iter()
        .take(7)
        .map(|cell| cell.rune)
        .collect();
    assert_eq!(runes, "legacyé".chars().map(u32::from).collect::<Vec<_>>());
}

#[test]
fn oversized_native_metadata_remains_state_while_transport_rejects_the_packet() {
    let mut tab = Tab::record(1, &vec![b'x'; worminal_protocol::MAX_PAYLOAD]);
    let bytes = tab.metadata(1).unwrap().to_vec();
    assert!(bytes.len() > worminal_protocol::MAX_PAYLOAD);
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut client = Client {
        id: 1,
        socket,
        pid: 0,
        ready: true,
        features: 0,
        decoder: Decoder::default(),
        output: Queue::new(CLIENT_QUEUE),
        needs_resync: false,
        selection: None,
    };
    client.output.append(b"previously-queued").unwrap();
    assert!(client
        .queue(worminal_protocol::WIRE_CATALOG, 0, &bytes)
        .is_err());
    assert_eq!(client.output.pending(), b"previously-queued");
}

#[test]
fn clean_metadata_reuses_storage_and_dirty_metadata_replaces_the_cache() {
    let mut tab = Tab::browser(1, b"https://first.test".to_vec());
    let first = tab.metadata(1).unwrap().to_vec();
    let storage = tab.catalog.as_ptr();
    for now in 2..10 {
        assert_eq!(tab.metadata(now).unwrap(), first);
        assert_eq!(tab.catalog.as_ptr(), storage);
        assert!(!tab.catalog_dirty);
        assert!(!tab.catalog_pending);
    }
    tab.url = Some(b"https://second.test".to_vec());
    tab.catalog_dirty = true;
    assert_ne!(tab.metadata(10).unwrap(), first);
    let updated = tab.catalog.clone();
    let storage = tab.catalog.as_ptr();
    assert_eq!(tab.metadata(11).unwrap(), updated);
    assert_eq!(tab.catalog.as_ptr(), storage);
}

#[test]
fn a_foreground_ssh_client_labels_the_tab_with_its_user() {
    // A shell symlinked as ssh waits on the terminal: a harmless foreground job whose argv[0] is an ssh client.
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../build/checks")
        .join(format!("ssh-foreground-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let ssh = directory.join("ssh");
    let _ = std::fs::remove_file(&ssh);
    std::os::unix::fs::symlink("/bin/sh", &ssh).unwrap();
    let mut fixture = Fixture::running(vec![ssh.into(), "-c".into(), "read line".into()]);
    let tab = &mut fixture.tab;
    let deadline = Instant::now() + Duration::from_secs(2);
    while tab.ssh_user.is_none() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
        tab.refresh_directory(1);
    }
    assert_eq!(tab.ssh_user.as_deref(), Some(&crate::ssh::local_user()[..]));
    assert_eq!(tab.label(), crate::ssh::local_user());
    assert!(tab.regroup && tab.catalog_dirty);
    let field =
        worminal_protocol::worp::events::directory_field(&tab.directory, tab.ssh_user.as_deref());
    assert!(
        field.contains(&0),
        "the catalog carries the user after a NUL"
    );
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn output_directory_checks_coalesce_without_delaying_periodic_checks() {
    let mut fixture = Fixture::new();
    let tab = &mut fixture.tab;
    let interval = worminal_protocol::WORKSPACE_DIRECTORY_INTERVAL_NS as i64;
    tab.refresh_directory_after_output(1);
    let actual = tab.directory.clone();
    assert!(!actual.is_empty());
    tab.directory = b"not-the-shell-directory".to_vec();
    for now in 2..1002 {
        tab.refresh_directory_after_output(now);
        assert_ne!(tab.directory, actual);
    }
    // The periodic poll must still observe a change even within the output gate.
    tab.refresh_directory(interval / 2);
    assert_eq!(tab.directory, actual);
    tab.directory = b"not-the-shell-directory".to_vec();
    tab.refresh_directory_after_output(interval);
    assert_ne!(tab.directory, actual);
    tab.refresh_directory_after_output(interval + interval / 2);
    assert_eq!(tab.directory, actual);
}

#[test]
fn catalog_lists_palette_overrides_only_through_the_last_one() {
    let mut fixture = Fixture::new();
    let tab = &mut fixture.tab;
    let none = 0u32.to_be_bytes();
    assert!(
        tab.metadata(1).unwrap().ends_with(&none),
        "theme defaults belong to each view, not the catalog"
    );
    tab.feed(b"\x1b]4;1;#123456\x07", 2).unwrap();
    let mut tail = 2u32.to_be_bytes().to_vec();
    tail.extend(0u32.to_be_bytes());
    tail.extend(7u32.to_be_bytes());
    tail.extend(b"#123456");
    assert!(tab.metadata(3).unwrap().ends_with(&tail));
    tab.feed(b"\x1b]104;1\x07", 4).unwrap();
    assert!(tab.metadata(5).unwrap().ends_with(&none));
}
