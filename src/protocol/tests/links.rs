use worminal_protocol::{
    decode_link_runs, encode_link_runs, link_runs, LinkRun, Payload, LINK_ROW_MAX, LINK_URI_MAX,
};

fn run(column: u32, count: u32, uri: &str) -> LinkRun {
    LinkRun {
        column,
        count,
        uri: uri.into(),
    }
}
fn raw(runs: &[(u32, u32, &[u8])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (column, count, uri) in runs {
        bytes.extend_from_slice(&column.to_be_bytes());
        bytes.extend_from_slice(&count.to_be_bytes());
        bytes.extend_from_slice(&(uri.len() as u32).to_be_bytes());
        bytes.extend_from_slice(uri);
    }
    bytes
}
fn decode(bytes: &[u8], columns: u32) -> std::io::Result<Vec<LinkRun>> {
    decode_link_runs(Payload::new(bytes), columns)
}

#[test]
fn runs_round_trip_and_empty_clears() {
    let runs = vec![run(0, 2, "http://a"), run(2, 1, "http://b"), run(5, 3, "x")];
    assert_eq!(decode(&encode_link_runs(&runs).unwrap(), 8).unwrap(), runs);
    assert_eq!(encode_link_runs(&[]).unwrap(), Vec::<u8>::new());
    assert_eq!(decode(&[], 8).unwrap(), []);
}

#[test]
fn decoder_rejects_every_malformed_run_list() {
    let long = "u".repeat(LINK_URI_MAX + 1);
    let max = "u".repeat(LINK_URI_MAX);
    assert!(decode(&raw(&[(0, 1, max.as_bytes())]), 8).is_ok());
    for (name, bytes) in [
        ("overlap", raw(&[(0, 3, b"a"), (2, 1, b"b")])),
        ("order", raw(&[(4, 1, b"a"), (1, 1, b"b")])),
        ("past the row", raw(&[(6, 3, b"a")])),
        ("column past the row", raw(&[(9, 1, b"a")])),
        ("overflowing count", raw(&[(1, u32::MAX, b"a")])),
        ("empty run", raw(&[(0, 0, b"a")])),
        ("empty URI", raw(&[(0, 1, b"")])),
        ("long URI", raw(&[(0, 1, long.as_bytes())])),
        ("not UTF-8", raw(&[(0, 1, &[0xff, 0xfe])])),
        ("truncated URI", raw(&[(0, 1, b"abc")])[..14].to_vec()),
        ("trailing bytes", [raw(&[(0, 1, b"a")]), vec![0]].concat()),
    ] {
        assert!(decode(&bytes, 8).is_err(), "{name}");
    }
}

#[test]
fn runs_merge_adjacent_ids_and_skip_unusable_targets() {
    let uri = |id: u16| match id {
        1 => Some(&b"http://one"[..]),
        2 => Some(&b"http://two"[..]),
        3 => Some(&[0xff][..]),
        _ => None,
    };
    assert_eq!(
        link_runs(&[0, 1, 1, 2, 3, 3, 9, 1], uri),
        [
            run(1, 2, "http://one"),
            run(3, 1, "http://two"),
            run(7, 1, "http://one")
        ]
    );
    // Bounded rows keep the leading runs only.
    let long = "u".repeat(LINK_URI_MAX);
    let ids: Vec<u16> = (1..=20).collect();
    let runs = link_runs(&ids, |_| Some(long.as_bytes()));
    assert!(!runs.is_empty() && runs.len() < 20);
    assert!(encode_link_runs(&runs).unwrap().len() <= LINK_ROW_MAX);
}
