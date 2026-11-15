use crate::{Engine, OutputPosition};
fn feed(engine: &mut Engine, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        let (used, _) = engine.feed(bytes, 1).unwrap();
        assert!(used > 0);
        bytes = &bytes[used..];
    }
}
#[test]
fn output_anchors_preserve_unicode_wraps_selection_and_scrolled_view() {
    let mut engine = Engine::new(8, 3).unwrap();
    let start = engine.output_position().unwrap();
    feed(&mut engine, "123456789\r\nα中\r\nthree\r\nend".as_bytes());
    let end = engine.output_position().unwrap();
    engine.scroll(true, 2).unwrap();
    engine.select(0, 0, true, 0, false, false).unwrap();
    engine.select(3, 1, false, 0, false, true).unwrap();
    let selection = engine.selection().unwrap();
    let frame = engine.frame(1).unwrap();
    let (bytes, next, flags) = engine.output(start, end).unwrap();
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "12345678\n9\nα中\nthree\nend\n"
    );
    assert_eq!(flags, 0);
    assert_eq!(next, end);
    assert_eq!(engine.selection().unwrap(), selection);
    assert_eq!(engine.frame(1).unwrap(), frame);
}
#[test]
fn output_reports_eviction_and_paginates_without_skipping_rows() {
    let mut engine = Engine::new(8, 3).unwrap();
    let start = engine.output_position().unwrap();
    // Exceed the configured default retained history without allocating a second log.
    feed(&mut engine, "row\r\n".repeat(11000).as_bytes());
    let end = engine.output_position().unwrap();
    let (first, mut next, flags) = engine.output(start, end).unwrap();
    assert_eq!(flags, 3);
    let mut count = first.iter().filter(|b| **b == b'\n').count();
    loop {
        let (bytes, position, flags) = engine.output(next, end).unwrap();
        assert_eq!(flags & 1, 0);
        count += bytes.iter().filter(|b| **b == b'\n').count();
        next = position;
        if flags & 2 == 0 {
            break;
        }
    }
    assert_eq!(next, end);
    assert_eq!(count, 10002); // 10,000 retained rows plus two completed screen rows.
}
#[test]
fn output_invalidates_layout_reset_alternate_and_backward_edit_anchors() {
    for escape in [b"\x1b[2J".as_slice(), b"\x1bc", b"\x1b[?1049h", b"\x1b[1A"] {
        let mut engine = Engine::new(12, 3).unwrap();
        feed(&mut engine, b"one\r\ntwo");
        let start = engine.output_position().unwrap();
        feed(&mut engine, escape);
        let end = engine.output_position().unwrap();
        assert_ne!(start.epoch, end.epoch, "{escape:?}");
        assert_eq!(engine.output(start, end).unwrap().2 & 1, 1);
    }
    let mut engine = Engine::new(12, 3).unwrap();
    let start = engine.output_position().unwrap();
    engine.resize(8, 4).unwrap();
    let end = engine.output_position().unwrap();
    assert_ne!(start.epoch, end.epoch);
    assert_eq!(engine.output(start, end).unwrap().2 & 1, 1);
}
#[test]
fn command_end_columns_exclude_later_prompt_and_full_width_wrap_is_included() {
    let mut engine = Engine::new(8, 3).unwrap();
    feed(&mut engine, b"prompt>");
    let start = engine.output_position().unwrap();
    feed(&mut engine, b"12345678");
    let end = engine.output_position().unwrap();
    feed(&mut engine, b"LATER");
    assert_eq!(engine.output(start, end).unwrap().0, b"1\n2345678\n");
    let invalid = OutputPosition {
        epoch: end.epoch,
        row: end.row,
        column: 99,
    };
    assert_eq!(engine.output(start, invalid).unwrap().2 & 1, 1);
}
