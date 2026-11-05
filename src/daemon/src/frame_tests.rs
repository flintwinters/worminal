use super::*;
use worminal_protocol::{Decoder, Packet};

fn feed(engine: &mut Engine, mut bytes: &[u8], now: i64) {
    while !bytes.is_empty() {
        let (used, events) = engine.feed(bytes, now).unwrap();
        assert!(events.is_empty());
        assert!(used > 0);
        bytes = &bytes[used..];
    }
}

fn packets(queue: &Queue) -> Vec<Packet> {
    let mut bytes = queue.pending();
    let mut decoder = Decoder::default();
    let mut result = Vec::new();
    while !bytes.is_empty() {
        let (used, packet) = decoder.push(bytes).unwrap();
        bytes = &bytes[used..];
        if let Some(packet) = packet {
            result.push(packet);
        }
    }
    result
}

fn apply(replica: &mut Vec<u8>, packets: &[Packet], columns: usize, rows: usize) {
    let width = columns * WIRE_CELL_BYTES as usize;
    replica.resize(width * rows, 0);
    assert_eq!(packets.first().unwrap().kind, WIRE_FRAME);
    assert_eq!(packets.last().unwrap().kind, WIRE_FRAME_FINISH);
    for packet in packets {
        if packet.kind == WIRE_ROW {
            let row = u32::from_be_bytes(packet.body[..4].try_into().unwrap()) as usize;
            replica[row * width..(row + 1) * width].copy_from_slice(&packet.body[4..]);
        } else if packet.kind == WIRE_COPY_ROWS {
            let fields: Vec<_> = packet
                .body
                .chunks_exact(4)
                .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()) as usize)
                .collect();
            replica.copy_within(
                fields[0] * width..(fields[0] + fields[2]) * width,
                fields[1] * width,
            );
        }
    }
}

#[test]
fn queue_rejection_preserves_damage_and_the_last_delivered_baseline() {
    let mut engine = Engine::new(8, 4).unwrap();
    let mut screen = Screen::default();
    let mut output = Queue::new(4096);
    screen
        .replicate(&mut engine, &mut output, 1, false, true, false, 1)
        .unwrap();
    let old = screen.bytes.clone();
    feed(&mut engine, b"changed", 2);
    let mut blocked = Queue::new(2);
    blocked.append(b"ok").unwrap();
    assert!(screen
        .replicate(&mut engine, &mut blocked, 1, false, true, false, 2)
        .is_err());
    assert_eq!(blocked.pending(), b"ok");
    assert_eq!(screen.bytes, old);
    assert!(engine.row_dirty(0).unwrap());
    let mut delivered = Queue::new(4096);
    assert_eq!(
        screen
            .replicate(&mut engine, &mut delivered, 1, false, true, false, 2)
            .unwrap(),
        Outcome::Sent
    );
    let mut replica = old;
    apply(&mut replica, &packets(&delivered), 8, 4);
    assert_eq!(replica, screen.bytes);
    assert!(!engine.row_dirty(0).unwrap());
}

#[test]
fn scroll_moves_and_legacy_rows_reconstruct_the_same_viewport() {
    for enabled in [false, true] {
        let mut engine = Engine::new(8, 6).unwrap();
        let mut screen = Screen::default();
        let mut initial = Queue::new(4096);
        feed(&mut engine, b"a\r\nb\r\nc\r\nd\r\ne\r\nf", 1);
        screen
            .replicate(&mut engine, &mut initial, 1, false, enabled, false, 1)
            .unwrap();
        let mut replica = screen.bytes.clone();
        feed(&mut engine, b"\r\ng", 2);
        let mut delta = Queue::new(4096);
        screen
            .replicate(&mut engine, &mut delta, 1, false, enabled, false, 2)
            .unwrap();
        let packets = packets(&delta);
        assert_eq!(
            packets.iter().any(|packet| packet.kind == WIRE_COPY_ROWS),
            enabled
        );
        apply(&mut replica, &packets, 8, 6);
        assert_eq!(replica, screen.bytes);
        let mut complete = Screen::default();
        complete
            .replicate(&mut engine, &mut Queue::new(4096), 1, true, false, false, 2)
            .unwrap();
        assert_eq!(replica, complete.bytes);
    }
}

#[test]
fn unchanged_damage_suppresses_frames_but_explicit_scroll_acknowledges() {
    let mut engine = Engine::new(8, 4).unwrap();
    let mut screen = Screen::default();
    screen
        .replicate(
            &mut engine,
            &mut Queue::new(4096),
            1,
            false,
            false,
            false,
            1,
        )
        .unwrap();
    feed(&mut engine, b"\x1b[2J", 2);
    let mut output = Queue::new(4096);
    assert_eq!(
        screen
            .replicate(&mut engine, &mut output, 1, false, true, false, 2)
            .unwrap(),
        Outcome::Unchanged
    );
    assert!(output.is_empty());
    assert!(!engine.row_dirty(0).unwrap());
    assert_eq!(
        screen
            .replicate(&mut engine, &mut output, 1, false, true, true, 3)
            .unwrap(),
        Outcome::Sent
    );
    assert_eq!(packets(&output).len(), 2);
}

#[test]
fn deferred_full_claim_retains_damage_and_rebuilds_every_row() {
    let mut engine = Engine::new(8, 4).unwrap();
    let mut screen = Screen::default();
    screen
        .replicate(
            &mut engine,
            &mut Queue::new(4096),
            1,
            false,
            false,
            false,
            1,
        )
        .unwrap();
    feed(&mut engine, b"\x1b[?2026hheld", 2);
    let mut output = Queue::new(4096);
    assert_eq!(
        screen
            .replicate(&mut engine, &mut output, 1, true, false, false, 3)
            .unwrap(),
        Outcome::Held
    );
    assert!(screen.bytes.is_empty());
    assert!(engine.row_dirty(0).unwrap());
    assert!(output.is_empty());
    feed(&mut engine, b"\x1b[?2026l", 4);
    assert_eq!(
        screen
            .replicate(&mut engine, &mut output, 1, false, false, false, 4)
            .unwrap(),
        Outcome::Sent
    );
    assert_eq!(
        packets(&output)
            .iter()
            .filter(|packet| packet.kind == WIRE_ROW)
            .count(),
        4
    );
}

#[test]
fn full_frame_matches_independently_encoded_cells_and_packet_order() {
    let mut engine = Engine::new(8, 4).unwrap();
    feed(
        &mut engine,
        "\x1b[1;38;2;12;34;56mAé\r\n\x1b[0mwide界".as_bytes(),
        1,
    );
    let frame = engine.frame(1).unwrap();
    let mut body = Buffer::default();
    for value in [
        frame.columns, frame.rows, frame.cursor_x, frame.cursor_y, frame.scroll, frame.alternate,
    ] {
        body.put_u32(value as u32).unwrap();
    }
    let mut expected = Buffer::default();
    expected
        .append_packet(&Packet { kind: WIRE_FRAME, tab: 7, body: body.into_bytes() })
        .unwrap();
    let mut cells = Vec::new();
    for row in 0..frame.rows {
        let mut body = Buffer::default();
        body.put_u32(row as u32).unwrap();
        engine.row(row, &mut cells).unwrap();
        for cell in &cells {
            for value in [cell.rune, cell.attributes, cell.foreground, cell.background] {
                body.put_u32(value).unwrap();
            }
        }
        expected
            .append_packet(&Packet { kind: WIRE_ROW, tab: 7, body: body.into_bytes() })
            .unwrap();
    }
    expected
        .append_packet(&Packet { kind: WIRE_FRAME_FINISH, tab: 7, body: Vec::new() })
        .unwrap();
    let mut output = Queue::new(4096);
    Screen::default()
        .replicate(&mut engine, &mut output, 7, true, true, false, 1)
        .unwrap();
    assert_eq!(output.pending(), expected.as_bytes());
}
