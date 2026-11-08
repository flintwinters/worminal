use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
};
use worminal_protocol::{Buffer, Decoder, Packet, Payload, MAX_PAYLOAD, WIRE_HELLO, WIRE_VERSION};

#[test]
fn c_and_rust_frames_are_identical_across_byte_chunks() {
    let mut body = Buffer::default();
    body.put_u32(42).unwrap();
    body.put_string(b"path-with-\xff-byte").unwrap();
    let packets = [
        Packet {
            kind: WIRE_HELLO,
            tab: 7,
            body: body.into_bytes(),
        },
        Packet {
            kind: 0xfedcba98,
            tab: 0,
            body: Vec::new(),
        },
    ];
    let bytes: Vec<u8> = packets
        .iter()
        .flat_map(|packet| packet.encode().unwrap())
        .collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut reference = Command::new(root.join("build/wire_reference"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("use manage.py check-rust-workspace");
    reference.stdin.take().unwrap().write_all(&bytes).unwrap();
    let output = reference.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, bytes);
    let mut decoder = Decoder::default();
    let mut actual = Vec::new();
    for byte in bytes {
        let (used, packet) = decoder.push(&[byte]).unwrap();
        assert_eq!(used, 1);
        actual.extend(packet);
    }
    assert_eq!(actual, packets);
}

#[test]
fn rejected_frames_poison_decoder_before_accepting_more_bytes() {
    for (version, length) in [
        (WIRE_VERSION + 1, 0),
        (WIRE_VERSION, MAX_PAYLOAD as u32 + 1),
    ] {
        let bytes: Vec<u8> = [version, 1, 2, length]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect();
        let mut decoder = Decoder::default();
        assert!(decoder.push(&bytes).is_err());
        assert!(decoder
            .push(
                &Packet {
                    kind: 1,
                    tab: 0,
                    body: Vec::new()
                }
                .encode()
                .unwrap()
            )
            .is_err());
    }
}

#[test]
fn bounded_publication_and_payload_validation() {
    let mut output = Buffer::with_limit(8);
    output.put_u32(1).unwrap();
    assert!(output.put_string(b"x").is_err());
    assert_eq!(output.as_bytes(), 1u32.to_be_bytes());
    assert!(output
        .append_packet(&Packet {
            kind: 1,
            tab: 0,
            body: Vec::new()
        })
        .is_err());
    assert_eq!(output.as_bytes(), 1u32.to_be_bytes());
    let mut payload = Payload::new(&[0, 0, 0, 2, b'a']);
    assert!(payload.string().is_err());
    let mut payload = Payload::new(&[0, 0, 0, 0, 1]);
    assert_eq!(payload.string().unwrap(), b"");
    assert!(payload.finish().is_err());
}

#[test]
fn multiple_frames_leave_the_next_packet_to_the_caller() {
    let bytes = Packet {
        kind: 1,
        tab: 2,
        body: b"abc".to_vec(),
    }
    .encode()
    .unwrap();
    let both = [bytes.as_slice(), bytes.as_slice()].concat();
    let mut decoder = Decoder::default();
    let (used, first) = decoder.push(&both).unwrap();
    assert_eq!(used, bytes.len());
    let (used_again, second) = decoder.push(&both[used..]).unwrap();
    assert_eq!(used_again, bytes.len());
    assert_eq!(first, second);
}

#[test]
fn fragmented_packet_bodies_match_contiguous_encoding_and_reject_atomically() {
    let packet = Packet {
        kind: WIRE_HELLO,
        tab: 7,
        body: b"ab\0\xffcd".to_vec(),
    };
    let expected = packet.encode().unwrap();
    for split in 0..=packet.body.len() {
        let parts = [&packet.body[..split], &[][..], &packet.body[split..]];
        let mut output = Buffer::with_limit(expected.len());
        output.reserve(expected.len()).unwrap();
        assert!(output.as_bytes().is_empty());
        output
            .append_packet_parts(packet.kind, packet.tab, &parts)
            .unwrap();
        assert_eq!(output.as_bytes(), expected);
        let before = output.as_bytes().to_vec();
        assert!(output.append_packet_parts(1, 0, &[]).is_err());
        assert_eq!(output.as_bytes(), before);
        let mut limited = Buffer::with_limit(expected.len() - 1);
        limited.put_u32(42).unwrap();
        assert!(limited
            .append_packet_parts(packet.kind, packet.tab, &parts)
            .is_err());
        assert!(limited.reserve(expected.len()).is_err());
        assert_eq!(limited.as_bytes(), 42u32.to_be_bytes());
    }
    let large = vec![0; MAX_PAYLOAD];
    let mut output = Buffer::with_limit(usize::MAX);
    output.put_u32(42).unwrap();
    assert!(output.append_packet_parts(1, 0, &[&large, b"x"]).is_err());
    assert_eq!(output.as_bytes(), 42u32.to_be_bytes());
}
