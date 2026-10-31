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
        let reaper = ChildReaper::default();
        let mut launch = Launch::restore(Vec::new());
        launch.arguments = vec!["/bin/cat".into()];
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
    let before = tab.metadata(1).unwrap();
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
