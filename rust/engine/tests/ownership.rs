use worminal_engine::{Engine, TransportState};

fn feed(engine: &mut Engine, mut bytes: &[u8], now: i64) -> Vec<Vec<u8>> {
    let mut effects = Vec::new();
    while !bytes.is_empty() {
        let (used, events) = engine.feed(bytes, now).unwrap();
        assert!(used > 0);
        effects.extend(events.into_iter().map(|event| event.data));
        bytes = &bytes[used..];
    }
    effects
}

#[test]
fn independent_instances_and_owned_rows() {
    let mut first = Engine::new(12, 3).unwrap();
    let mut second = Engine::new(8, 2).unwrap();
    assert_eq!(
        feed(&mut first, b"A\x1b]2;title\x07B", 1),
        vec![b"title".to_vec()]
    );
    feed(&mut second, b"xyz", 1);
    let (_, retained) = first.changed_rows(1).unwrap();
    assert_eq!(retained[0].1[0].rune, u32::from('A'));
    assert_eq!(retained[0].1[1].rune, u32::from('B'));
    assert!(first.changed_rows(1).unwrap().1.is_empty());
    drop(first);
    // Returned rows survive destruction of their originating C allocation.
    assert_eq!(retained[0].1[1].rune, u32::from('B'));
    assert_eq!(
        second.changed_rows(1).unwrap().1[0].1[0].rune,
        u32::from('x')
    );
}

#[test]
fn snapshot_retains_parser_and_application_fragments() {
    let mut original = Engine::new(12, 4).unwrap();
    feed(
        &mut original,
        b"one\r\ntwo\r\nthree\r\nfour\r\nfive\x1b[31",
        1,
    );
    original.resize(7, 5).unwrap();
    let mut transport = TransportState::default();
    transport.pending_bytes = 2;
    transport.pending[..2].copy_from_slice(&[0xe2, 0x82]);
    let snapshot = original.export(&transport).unwrap();
    let (mut restored, imported) = Engine::import(&snapshot).unwrap();
    assert_eq!(imported.pending_bytes, 2);
    assert_eq!(&imported.pending[..2], &[0xe2, 0x82]);
    feed(&mut original, b"mX", 2);
    feed(&mut restored, b"mX", 2);
    assert_eq!(
        original.changed_rows(2).unwrap(),
        restored.changed_rows(2).unwrap()
    );
    assert!(Engine::import(&snapshot[..snapshot.len() - 1]).is_err());
}

#[test]
fn synchronized_rows_remain_dirty_until_the_deadline() {
    let mut engine = Engine::new(8, 3).unwrap();
    engine.changed_rows(1).unwrap();
    feed(&mut engine, b"\x1b[?2026hX", 100);
    let (frame, rows) = engine.changed_rows(100).unwrap();
    assert_eq!(frame.redraw_deadline, 150000100);
    assert!(rows.is_empty());
    let (frame, rows) = engine.changed_rows(150000100).unwrap();
    assert_eq!(frame.redraw_deadline, 0);
    assert_eq!(rows[0].1[0].rune, u32::from('X'));
}

#[test]
fn incomplete_utf8_is_returned_to_the_transport() {
    let mut engine = Engine::new(8, 3).unwrap();
    assert_eq!(engine.feed(&[b'a', 0xe2, 0x82], 1).unwrap().0, 1);
    assert_eq!(engine.feed(&[0xe2, 0x82, 0xac], 1).unwrap().0, 3);
    let rows = engine.changed_rows(1).unwrap().1;
    assert_eq!(rows[0].1[1].rune, 0x20ac);
    assert!(engine.resize(0, 1).is_err());
    assert!(Engine::new(-1, 3).is_err());
}

#[test]
fn selection_input_modes_and_alternate_permission() {
    let mut engine = Engine::new(8, 3).unwrap();
    feed(&mut engine, b"abc", 1);
    engine.select(0, 0, true, 0, false, false).unwrap();
    engine.select(1, 0, false, 0, false, false).unwrap();
    engine.select(1, 0, false, 0, false, true).unwrap();
    assert_eq!(engine.selection().unwrap(), b"ab");
    assert!(engine.selected(1, 0).unwrap());
    engine.clear_selection().unwrap();
    assert!(!engine.selected(1, 0).unwrap());
    feed(&mut engine, b"\x1b[12l\x1b[20h", 2);
    let events = engine.input(b"q\r", true, 3).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, worminal_engine::WORMINAL_WRITE);
    assert_eq!(events[0].data, b"q\r\n");
    engine.allow_alternate(false).unwrap();
    feed(&mut engine, b"\x1b[?1049h", 4);
    assert_eq!(engine.changed_rows(4).unwrap().0.alternate, 0);
    engine.allow_alternate(true).unwrap();
    feed(&mut engine, b"\x1b[?1049h", 5);
    assert_eq!(engine.changed_rows(5).unwrap().0.alternate, 1);
}

#[test]
fn full_rows_and_embedded_snapshot_prefixes_are_explicit() {
    let mut engine = Engine::new(8, 3).unwrap();
    feed(&mut engine, b"abc", 1);
    assert_eq!(engine.changed_rows(1).unwrap().1.len(), 3);
    assert!(engine.changed_rows(1).unwrap().1.is_empty());
    assert_eq!(engine.all_rows(1).unwrap().1.len(), 3);
    let snapshot = engine.export(&TransportState::default()).unwrap();
    let mut bundle = snapshot.clone();
    bundle.extend_from_slice(b"next terminal metadata");
    let (mut restored, _, consumed) = Engine::import_prefix(&bundle).unwrap();
    assert_eq!(consumed, snapshot.len());
    assert_eq!(restored.all_rows(2).unwrap().1[0].1[0].rune, u32::from('a'));
    let printed = restored
        .control(worminal_engine::WORMINAL_PRINT_SCREEN)
        .unwrap();
    assert_eq!(printed.len(), 1);
    assert_eq!(printed[0].kind, worminal_engine::WORMINAL_PRINT);
    assert_eq!(printed[0].data, b"abc\n\n\n");
}
