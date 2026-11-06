use worminal_engine::{paste_begin, paste_end, PasteFilter};

fn filtered(bracketed: bool, chunks: &[&[u8]]) -> Vec<u8> {
    let mut filter = PasteFilter::new(bracketed);
    let mut output = Vec::new();
    for chunk in chunks {
        let mut bytes = chunk.to_vec();
        let written = filter.filter(&mut bytes);
        output.extend_from_slice(&bytes[..written]);
    }
    output
}

#[test]
fn paste_rules_are_independent_of_chunk_boundaries() {
    let cases: &[(bool, &[u8], &[u8])] = &[
        (false, b"one\r\n\ntwo\n", b"one\r\rtwo\r"),
        (true, b"one\r\n\x1b[201~\x03two", b"one\r\n[201~two"),
        (false, b"\r\r\n\n\x1b\x03\0", b"\r\r\r\x1b\x03\0"),
        (true, "λ中\r\n".as_bytes(), "λ中\r\n".as_bytes()),
        (false, b"", b""),
    ];
    for &(bracketed, input, expected) in cases {
        for split in 0..=input.len() {
            assert_eq!(filtered(bracketed, &[&input[..split], b"", &input[split..]]), expected);
        }
        let bytes: Vec<_> = input.chunks(1).collect();
        assert_eq!(filtered(bracketed, &bytes), expected);
    }
    assert_eq!(paste_begin(), b"\x1b[200~");
    assert_eq!(paste_end(), b"\x1b[201~");
}
