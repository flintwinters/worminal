//! The saved window layout: format round trip, damaged files, and fitting a layout to the tabs that exist.
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use worminal_workspace::layout_state::{self, Node, Saved, Window};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("layout-{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Self(root.join("layout.json"))
    }
}

fn pane(tabs: &[u32], selected: u32) -> Node {
    Node::Pane {
        tabs: tabs.to_vec(),
        selected: Some(selected),
    }
}

fn split(side_by_side: bool, ratio: u16, first: Node, second: Node) -> Node {
    Node::Split {
        side_by_side,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    }
}

fn grid() -> Node {
    split(
        true,
        300,
        split(false, 500, pane(&[0], 0), pane(&[1, 4], 4)),
        pane(&[2, 3], 2),
    )
}

fn saved() -> Saved {
    Saved {
        windows: vec![Window {
            id: 7,
            geometry: [10, 20, 1000, 600],
            maximized: false,
            tree: grid(),
            focused: 1,
        }],
    }
}

#[test]
fn a_layout_survives_a_write_and_read_with_private_permissions() {
    let fixture = Fixture::new("round-trip");
    assert_eq!(layout_state::load(&fixture.0).unwrap(), None);
    layout_state::store(&fixture.0, &saved()).unwrap();
    assert_eq!(layout_state::load(&fixture.0).unwrap(), Some(saved()));
    assert_eq!(
        fs::metadata(&fixture.0).unwrap().permissions().mode() & 0o777,
        0o600
    );
    layout_state::forget(&fixture.0).unwrap();
    assert_eq!(layout_state::load(&fixture.0).unwrap(), None);
    layout_state::forget(&fixture.0).unwrap();
}

#[test]
fn a_damaged_file_is_reported_and_kept_never_overwritten() {
    for (name, text) in [
        ("garbage", "{not json".to_owned()),
        ("version", r#"{"version": 9, "windows": []}"#.to_owned()),
        (
            "ratio",
            String::from_utf8(
                Saved {
                    windows: vec![Window {
                        tree: split(true, 500, pane(&[0], 0), pane(&[1], 1)),
                        ..saved().windows[0].clone()
                    }],
                }
                .encode(),
            )
            .unwrap()
            .replace("500", "1000"),
        ),
        (
            "size",
            String::from_utf8(
                Saved {
                    windows: vec![Window {
                        geometry: [0, 0, 0, 600],
                        ..saved().windows[0].clone()
                    }],
                }
                .encode(),
            )
            .unwrap(),
        ),
        (
            "tab",
            String::from_utf8(
                Saved {
                    windows: vec![Window {
                        tree: pane(&[99999], 99999),
                        ..saved().windows[0].clone()
                    }],
                }
                .encode(),
            )
            .unwrap(),
        ),
    ] {
        let fixture = Fixture::new(name);
        fs::write(&fixture.0, &text).unwrap();
        assert!(
            layout_state::load(&fixture.0).is_err(),
            "{name} must not load"
        );
        assert!(
            layout_state::store(&fixture.0, &saved()).is_err(),
            "{name} must not be replaced"
        );
        assert_eq!(fs::read_to_string(&fixture.0).unwrap(), text, "{name}");
    }
}

#[test]
fn nesting_and_size_are_bounded() {
    let mut tree = pane(&[0], 0);
    for _ in 0..40 {
        tree = split(true, 500, tree, pane(&[1], 1));
    }
    let deep = Saved {
        windows: vec![Window {
            tree,
            ..saved().windows[0].clone()
        }],
    };
    let fixture = Fixture::new("deep");
    fs::write(&fixture.0, deep.encode()).unwrap();
    assert!(layout_state::load(&fixture.0).is_err());
}

#[test]
fn fitting_keeps_a_layout_whose_tabs_all_exist() {
    assert_eq!(grid().fit(5), grid());
}

#[test]
fn fitting_drops_missing_tabs_and_collapses_the_panes_they_leave_empty() {
    // Only positions 0 and 1 exist now: the right column held 2 and 3, so it collapses into its sibling.
    assert_eq!(
        grid().fit(2),
        split(false, 500, pane(&[0], 0), pane(&[1], 1))
    );
    // A pane keeps the tabs that remain and its selection moves to the first of them.
    assert_eq!(
        split(true, 300, pane(&[0], 0), pane(&[1, 4], 4)).fit(2),
        split(true, 300, pane(&[0], 0), pane(&[1], 1))
    );
    assert_eq!(
        split(true, 500, pane(&[0], 0), pane(&[5, 6], 5)).fit(1),
        pane(&[0], 0)
    );
    // Nothing fits: one pane with every tab that exists.
    assert_eq!(pane(&[7, 8], 7).fit(3), pane(&[0, 1, 2], 0));
}

#[test]
fn fitting_places_each_tab_once_and_gives_new_tabs_to_the_first_pane() {
    let tree = split(true, 500, pane(&[1, 1], 1), pane(&[1, 0], 0));
    // Position 1 stays in the first pane, so the second keeps only 0; tab 2 is new.
    assert_eq!(
        tree.fit(3),
        split(true, 500, pane(&[1, 2], 1), pane(&[0], 0))
    );
    assert_eq!(Node::pane(vec![]).fit(2), pane(&[0, 1], 0));
    assert_eq!(
        Node::pane(vec![]).fit(0),
        Node::Pane {
            tabs: vec![],
            selected: None
        }
    );
}

#[test]
fn paths_are_scoped_beside_the_recovery_file() {
    let root = PathBuf::from("/state");
    let one = layout_state::path(&root, b"a").unwrap();
    assert_ne!(one, layout_state::path(&root, b"b").unwrap());
    assert!(one.starts_with("/state/worminal"));
    assert!(layout_state::path(&PathBuf::from("relative"), b"a").is_err());
}
