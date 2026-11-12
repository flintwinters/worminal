use super::*;

fn cells() -> Vec<u8> {
    [65u32, 0xffff0001, 0x01020304, 0xaabbccdd]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect()
}
fn roundtrip(kind: EventKind) {
    let event = Event { tab: 7, kind };
    assert_eq!(decode(encode(&event).unwrap()).unwrap(), Some(event));
}
#[test]
fn every_event_roundtrips_and_catalog_keeps_filename_bytes() {
    let variants = [
        EventKind::Catalog(Catalog {
            closed: 2,
            fallback: 7,
            tabs: vec![TabRecord {
                id: 7,
                kind: TabKind::Terminal,
                url: vec![],
                mode: 99,
                cursor: 2,
                title: b"title\0suffix".to_vec(),
                directory: vec![b'/', 0xff],
                colors: vec![b"#112233".to_vec()],
            }],
        }),
        EventKind::FrameStart(FrameState {
            columns: 80,
            rows: 24,
            cursor_x: 3,
            cursor_y: 4,
            scroll: 9,
            alternate: true,
        }),
        EventKind::Row {
            row: 3,
            column: None,
            cells: CellBuffer::new(cells()).unwrap(),
        },
        EventKind::Row {
            row: 4,
            column: Some(2),
            cells: CellBuffer::new(cells()).unwrap(),
        },
        EventKind::CopyRows {
            source: 1,
            destination: 2,
            count: 3,
        },
        EventKind::FrameFinish,
        EventKind::Clipboard(b"clipboard".to_vec()),
        EventKind::Bell,
        EventKind::Notification(b"i=abc;hello;world".to_vec()),
        EventKind::Print(vec![0, 255]),
        EventKind::Released,
        EventKind::Error(b"error".to_vec()),
        EventKind::Created,
        EventKind::Capabilities(15),
        EventKind::ServiceInfo(123),
        EventKind::CommandDone,
        EventKind::Reply(super::super::Reply {
            success: true,
            json: "{\"ok\":true}".into(),
        }),
        EventKind::Selection {
            generation: 42,
            done: false,
            text: vec![255, 0],
        },
    ];
    for variant in variants {
        roundtrip(variant);
    }
}
#[test]
fn packed_rows_retain_received_allocation_and_exact_wire_fields() {
    let mut body = 5u32.to_be_bytes().to_vec();
    body.extend(cells());
    let pointer = body.as_ptr().wrapping_add(4);
    let event = decode(Packet {
        kind: WIRE_ROW,
        tab: 9,
        body,
    })
    .unwrap()
    .unwrap();
    let EventKind::Row { cells: packed, .. } = &event.kind else {
        panic!("row expected")
    };
    assert_eq!(packed.as_bytes().as_ptr(), pointer);
    assert_eq!(packed.iter().next().unwrap().attributes, 0xffff0001);
    let packet = encode(&event).unwrap();
    assert_eq!(packet.kind, WIRE_ROW);
    assert_eq!(
        packet.body,
        [5u32.to_be_bytes().as_slice(), cells().as_slice()].concat()
    );
}
#[test]
fn borrowed_frame_writer_matches_owned_binding() {
    let bytes = cells();
    let frame = FrameState {
        columns: 80,
        rows: 24,
        cursor_x: 0,
        cursor_y: 0,
        scroll: 0,
        alternate: false,
    };
    let pairs = [
        (FrameRef::Start(frame), EventKind::FrameStart(frame)),
        (
            FrameRef::Row {
                row: 3,
                column: Some(2),
                cells: &bytes,
            },
            EventKind::Row {
                row: 3,
                column: Some(2),
                cells: CellBuffer::new(bytes.clone()).unwrap(),
            },
        ),
        (
            FrameRef::CopyRows {
                source: 1,
                destination: 0,
                count: 2,
            },
            EventKind::CopyRows {
                source: 1,
                destination: 0,
                count: 2,
            },
        ),
        (FrameRef::Finish, EventKind::FrameFinish),
    ];
    for (borrowed, kind) in pairs {
        let mut actual = Buffer::default();
        append_frame(&mut actual, 42, borrowed).unwrap();
        let packet = encode(&Event { tab: 42, kind }).unwrap();
        let mut expected = Buffer::default();
        expected
            .append_packet_bytes(packet.kind, packet.tab, &packet.body)
            .unwrap();
        assert_eq!(actual.into_bytes(), expected.into_bytes());
    }
}
#[test]
fn malformed_events_fail_before_publication_and_unknown_events_are_ignored() {
    for (kind, body) in [
        (WIRE_FRAME, vec![0; 24]),
        (WIRE_ROW, vec![0; 5]),
        (WIRE_ROW, vec![0; 4]),
        (WIRE_ROW_SPAN, vec![0; 7]),
        (WIRE_FRAME_FINISH, vec![1]),
        (WIRE_COPY_ROWS, vec![0; 12]),
        (WIRE_CATALOG, vec![255; 12]),
        (
            WIRE_SELECT_ALL,
            [1u32.to_be_bytes(), 2u32.to_be_bytes()].concat(),
        ),
    ] {
        assert!(
            decode(Packet { kind, tab: 1, body }).is_err(),
            "kind {kind}"
        );
    }
    assert!(decode(Packet {
        kind: u32::MAX,
        tab: 1,
        body: vec![]
    })
    .unwrap()
    .is_none());
    assert!(append_frame(
        &mut Buffer::default(),
        1,
        FrameRef::Row {
            row: 0,
            column: Some(WORKSPACE_MAX_COLS - 1),
            cells: &[0; 32],
        }
    )
    .is_err());
}

#[test]
fn link_events_keep_runs_and_match_borrowed_frame_bytes() {
    let runs = vec![LinkRun {
        column: 2,
        count: 3,
        uri: "https://example.org/osc8".into(),
    }];
    roundtrip(EventKind::Links {
        row: 4,
        runs: runs.clone(),
    });
    roundtrip(EventKind::Links {
        row: 4,
        runs: vec![],
    });
    let encoded = encode_link_runs(&runs).unwrap();
    let mut borrowed = Buffer::default();
    append_frame(
        &mut borrowed,
        1,
        FrameRef::Links {
            row: 4,
            runs: &encoded,
        },
    )
    .unwrap();
    let owned = encode(&Event {
        tab: 1,
        kind: EventKind::Links { row: 4, runs },
    })
    .unwrap();
    assert_eq!(borrowed.into_bytes(), owned.encode().unwrap());
    for values in [
        [WORKSPACE_MAX_ROWS, 0, 1],
        [0, WORKSPACE_MAX_COLS, 1],
        [0, 0, 0],
    ] {
        let mut body = Buffer::default();
        for value in values {
            body.put_u32(value).unwrap();
        }
        body.put_string(b"uri").unwrap();
        assert!(decode(Packet {
            kind: WIRE_ROW_LINKS,
            tab: 1,
            body: body.into_bytes()
        })
        .is_err());
    }
}
