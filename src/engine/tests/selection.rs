use worminal_engine::{Engine, Selection};

fn feed(engine: &mut Engine, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let (used, _) = engine.feed(bytes, 1).unwrap();
        assert!(used > 0);
        bytes = &bytes[used..];
    }
}

#[test]
fn borrowed_selection_matches_owner_for_wrapping_unicode_rectangles_and_alternate_screen() {
    for alternate in [false, true] {
        for snap in [0, 1, 2] {
            for rectangular in [false, true] {
                for reverse in [false, true] {
                    let mut owner = Engine::new(12, 4).unwrap();
                    if alternate {
                        feed(&mut owner, b"\x1b[?1049h");
                    }
                    feed(
                        &mut owner,
                        "one-two λ中ABCwrapped\r\n   prefix\r\nlast".as_bytes(),
                    );
                    let (frame, rows) = owner.all_rows(1).unwrap();
                    let cells: Vec<_> = rows.into_iter().flat_map(|(_, cells)| cells).collect();
                    let mut borrowed = Selection::new(&frame).unwrap();
                    let (start, end) = if reverse {
                        ((5, 2), (0, 0))
                    } else {
                        ((0, 0), (5, 2))
                    };
                    owner
                        .select(start.0, start.1, true, snap, rectangular, false)
                        .unwrap();
                    borrowed
                        .select(&cells, start.0, start.1, true, snap, rectangular, false)
                        .unwrap();
                    for done in [false, true] {
                        owner
                            .select(end.0, end.1, false, snap, rectangular, done)
                            .unwrap();
                        borrowed
                            .select(&cells, end.0, end.1, false, snap, rectangular, done)
                            .unwrap();
                    }
                    assert_eq!(borrowed.text(&cells).unwrap(), owner.selection().unwrap());
                    let mut ranges = Vec::new();
                    borrowed.ranges(&mut ranges).unwrap();
                    for y in 0..frame.rows {
                        for x in 0..frame.columns {
                            let range = ranges[y as usize];
                            assert_eq!(
                                x >= range.first && x < range.end,
                                owner.selected(x, y).unwrap()
                            );
                        }
                    }
                    drop(owner);
                    // The borrowed selection retains no pointer into either the engine or caller allocation.
                    let replacement = cells.clone();
                    drop(cells);
                    assert!(!borrowed.text(&replacement).unwrap().is_empty());
                    assert!(borrowed.text(&replacement[..1]).is_err());
                    borrowed.clear().unwrap();
                    assert!(borrowed.text(&replacement).unwrap().is_empty());
                }
            }
        }
    }
}

#[test]
fn whole_buffer_export_preserves_selection_and_scroll() {
    let mut owner = Engine::new(6, 2).unwrap();
    feed(&mut owner, "abcdefgh\r\né中".as_bytes());
    owner.scroll(true, 1).unwrap();
    owner.select(1, 0, true, 0, false, false).unwrap();
    owner.select(3, 0, false, 0, false, true).unwrap();
    let selected = owner.selection().unwrap();
    assert_eq!(owner.all_text().unwrap(), "abcdefgh\né中\n".as_bytes());
    assert_eq!(owner.selection().unwrap(), selected);
    assert_eq!(owner.frame(1).unwrap().scroll, 1);
    feed(&mut owner, b"\x1b[?1049hALT");
    let alternate = owner.all_text().unwrap();
    assert!(alternate.windows(3).any(|part| part == b"ALT"));
    assert!(!alternate.windows(8).any(|part| part == b"abcdefgh"));
}
