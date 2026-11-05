use worminal_engine_sys::{
    ATTR_WIDE, ATTR_WDUMMY, ATTR_WRAP, WORMINAL_DEFAULT_BACKGROUND, WORMINAL_DEFAULT_FOREGROUND,
};
use worminal_frontend::viewport::Viewport;
use worminal_protocol::*;

fn packet(kind: u32, values: &[u32]) -> Packet {
    Packet {
        kind,
        tab: 1,
        body: values
            .iter()
            .flat_map(|value| value.to_be_bytes())
            .collect(),
    }
}
fn header(view: &mut Viewport, columns: u32, rows: u32) {
    assert!(view
        .packet(&packet(WIRE_FRAME, &[columns, rows, 0, 0, 0, 0]))
        .unwrap()
        .is_none());
}
fn row(view: &mut Viewport, row: u32, text: &str, attributes: u32) {
    let mut update = packet(WIRE_ROW, &[row]);
    row_packet_cells(&mut update.body, text, attributes);
    assert!(view.packet(&update).unwrap().is_none());
}
fn finish(view: &mut Viewport) -> worminal_frontend::viewport::Damage {
    view.packet(&packet(WIRE_FRAME_FINISH, &[]))
        .unwrap()
        .unwrap()
}
fn text(view: &Viewport, row: usize) -> String {
    view.row(row)
        .unwrap()
        .iter()
        .map(|cell| char::from_u32(cell.rune).unwrap())
        .collect()
}

#[test]
fn frames_publish_only_on_finish_and_malformed_or_interrupted_deltas_preserve_the_visible_frame() {
    let mut view = Viewport::new(4, 2).unwrap();
    assert!(view.cells().is_empty());
    header(&mut view, 4, 2);
    row(&mut view, 0, "old!", 0);
    assert!(view.cells().is_empty());
    assert!(finish(&mut view).full);
    let visible = view.frame();
    header(&mut view, 3, 1);
    row(&mut view, 0, "new", 0);
    assert_eq!(view.frame(), visible);
    assert_eq!(text(&view, 0), "old!");
    assert!(view.packet(&packet(WIRE_COPY_ROWS, &[0, 0, 3])).is_err());
    assert_eq!(text(&view, 0), "old!");
    assert!(view.packet(&packet(WIRE_FRAME_FINISH, &[])).is_err());
    header(&mut view, 4, 2);
    row(&mut view, 0, "lost", 0);
    view.release();
    assert_eq!(text(&view, 0), "old!");
    assert!(!view.live());
    header(&mut view, 4, 2);
    row(&mut view, 0, "next", 0);
    assert!(finish(&mut view).full);
    assert_eq!(text(&view, 0), "next");
    header(&mut view, 4, 2);
    assert!(!finish(&mut view).changed());
    header(&mut view, 4, 2);
    row(&mut view, 0, "next", 0);
    assert!(!finish(&mut view).changed());
}

#[test]
fn overlapping_row_moves_and_cursor_damage_match_the_wire_contract() {
    let mut view = Viewport::new(1, 4).unwrap();
    header(&mut view, 1, 4);
    for (y, line) in ["a", "b", "c", "d"].iter().enumerate() {
        row(&mut view, y as u32, line, 0);
    }
    finish(&mut view);
    header(&mut view, 1, 4);
    view.packet(&packet(WIRE_COPY_ROWS, &[0, 1, 3])).unwrap();
    row(&mut view, 0, "X", 0);
    finish(&mut view);
    assert_eq!((0..4).map(|y| text(&view, y)).collect::<String>(), "Xabc");
    view.packet(&packet(WIRE_FRAME, &[1, 4, 0, 3, 0, 0]))
        .unwrap();
    view.packet(&packet(WIRE_COPY_ROWS, &[1, 0, 3])).unwrap();
    let damage = finish(&mut view);
    assert_eq!(damage.rows, [0, 1, 2, 3]);
    assert_eq!((0..4).map(|y| text(&view, y)).collect::<String>(), "abcc");
}

#[test]
fn imported_resize_crops_wide_cells_without_reflow_and_selection_borrows_rust_rows() {
    let mut view = Viewport::new(5, 2).unwrap();
    header(&mut view, 5, 2);
    row(&mut view, 0, "abcde", ATTR_WRAP);
    row(&mut view, 1, "ABCDE", ATTR_WIDE);
    finish(&mut view);
    view.select(0, 0, true, 0, false, false).unwrap();
    view.select(2, 0, false, 0, false, false).unwrap();
    view.select(2, 0, false, 0, false, true).unwrap();
    assert_eq!(view.selection_text().unwrap(), b"abc");
    header(&mut view, 3, 2);
    finish(&mut view);
    assert_eq!(text(&view, 0), "abc");
    assert_eq!(text(&view, 1), "AB ");
    assert_eq!(view.row(0).unwrap()[2].attributes & ATTR_WRAP, 0);
    assert!(view.selection_text().unwrap().is_empty());
    assert!(!view.selected(0, 0));
}

#[test]
fn malformed_headers_rows_and_unbounded_updates_never_publish() {
    let mut view = Viewport::new(1, 1).unwrap();
    for values in [
        [0, 1, 0, 0, 0, 0],
        [1, 201, 0, 0, 0, 0],
        [1, 1, 1, 0, 0, 0],
        [1, 1, 0, 0, u32::MAX, 0],
        [1, 1, 0, 0, 0, 2],
    ] {
        assert!(view.packet(&packet(WIRE_FRAME, &values)).is_err());
        assert!(view.cells().is_empty());
    }
    header(&mut view, 1, 1);
    assert!(view.packet(&packet(WIRE_ROW, &[0, 65])).is_err());
    header(&mut view, 1, 1);
    assert!(view
        .packet(&packet(WIRE_FRAME, &[1, 1, 0, 0, 0, 0]))
        .is_err());
    header(&mut view, 1, 1);
    let copy = packet(WIRE_COPY_ROWS, &[0, 0, 1]);
    let mut rejected = false;
    for _ in 0..MAX_PAYLOAD {
        if view.packet(&copy).is_err() {
            rejected = true;
            break;
        }
    }
    assert!(rejected);
    assert!(view.cells().is_empty());
    header(&mut view, WORKSPACE_MAX_COLS, WORKSPACE_MAX_ROWS);
    let expensive = packet(WIRE_COPY_ROWS, &[0, 0, WORKSPACE_MAX_ROWS]);
    let mut rejected_work = false;
    for _ in 0..100 {
        if view.packet(&expensive).is_err() {
            rejected_work = true;
            break;
        }
    }
    assert!(
        rejected_work,
        "small row-move packets must not create unbounded finish work"
    );
    assert!(view.cells().is_empty());
}

fn span(view: &mut Viewport, row: u32, column: u32, text: &str, attributes: u32) {
    let mut update = packet(WIRE_ROW_SPAN, &[row, column]);
    row_packet_cells(&mut update.body, text, attributes);
    view.packet(&update).unwrap();
}

fn row_packet_cells(body: &mut Vec<u8>, text: &str, attributes: u32) {
    for rune in text.chars() {
        for value in [u32::from(rune), attributes, WORMINAL_DEFAULT_FOREGROUND, WORMINAL_DEFAULT_BACKGROUND] {
            body.extend_from_slice(&value.to_be_bytes());
        }
    }
}

#[test]
fn spans_are_atomic_preserve_neighbors_and_follow_overlapping_moves() {
    let mut view = Viewport::new(5, 3).unwrap();
    header(&mut view, 5, 3);
    row(&mut view, 0, "abcde", 0);
    row(&mut view, 1, "ABCDE", 0);
    row(&mut view, 2, "12345", 0);
    finish(&mut view);
    header(&mut view, 5, 3);
    view.packet(&packet(WIRE_COPY_ROWS, &[0, 1, 2])).unwrap();
    span(&mut view, 1, 1, "界", ATTR_WIDE);
    span(&mut view, 1, 2, " ", ATTR_WDUMMY);
    span(&mut view, 2, 4, "é", ATTR_WRAP);
    assert_eq!(text(&view, 1), "ABCDE", "spans must remain private until finish");
    let damage = finish(&mut view);
    assert_eq!(damage.rows, [1, 2]);
    assert_eq!(text(&view, 1), "a界 de");
    assert_eq!(text(&view, 2), "ABCDé");
    assert_eq!(view.row(1).unwrap()[1].attributes, ATTR_WIDE);
    assert_eq!(view.row(2).unwrap()[4].attributes, ATTR_WRAP);
    header(&mut view, 5, 3);
    span(&mut view, 2, 4, "é", ATTR_WRAP);
    assert!(!finish(&mut view).changed());
}

#[test]
fn malformed_spans_discard_the_entire_pending_frame() {
    let mut view = Viewport::new(3, 1).unwrap();
    header(&mut view, 3, 1);
    row(&mut view, 0, "old", 0);
    finish(&mut view);
    for (y, column, cells) in [(1, 0, 1), (0, 3, 1), (0, 2, 2), (0, 0, 0), (0, u32::MAX, 1)] {
        header(&mut view, 3, 1);
        span(&mut view, 0, 0, "X", 0);
        let mut bad = packet(WIRE_ROW_SPAN, &[y, column]);
        row_packet_cells(&mut bad.body, &"Y".repeat(cells), 0);
        assert!(view.packet(&bad).is_err());
        assert!(view.packet(&packet(WIRE_FRAME_FINISH, &[])).is_err());
        assert_eq!(text(&view, 0), "old");
    }
    header(&mut view, 3, 1);
    let mut truncated = packet(WIRE_ROW_SPAN, &[0, 0, 65]);
    truncated.body.push(0);
    assert!(view.packet(&truncated).is_err());
    assert_eq!(text(&view, 0), "old");
}
