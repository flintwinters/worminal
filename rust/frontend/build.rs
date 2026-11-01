//! Compile the shared C key table into Rust; reject unsupported syntax rather than drifting silently.
use std::{env, fs, path::PathBuf};

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Number(i32),
    Bytes(Vec<u8>),
    Punct(u8),
}

fn tokens(input: &[u8]) -> Vec<Token> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < input.len() {
        let c = input[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if input[i..].starts_with(b"/*") {
            i += 2;
            while !input[i..].starts_with(b"*/") {
                assert!(i < input.len(), "unterminated keymap comment");
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == b'"' {
            i += 1;
            let mut bytes = Vec::new();
            while input[i] != b'"' {
                if input[i] != b'\\' {
                    bytes.push(input[i]);
                    i += 1;
                    continue;
                }
                i += 1;
                let escape = input[i];
                i += 1;
                let value = match escape {
                    b'0'..=b'7' => {
                        let mut value = u32::from(escape - b'0');
                        for _ in 0..2 {
                            if i == input.len() || !(b'0'..=b'7').contains(&input[i]) {
                                break;
                            }
                            value = value * 8 + u32::from(input[i] - b'0');
                            i += 1;
                        }
                        u8::try_from(value).expect("keymap octal byte")
                    }
                    b'r' => b'\r',
                    b'n' => b'\n',
                    b't' => b'\t',
                    b'\\' => b'\\',
                    b'"' => b'"',
                    _ => panic!("unsupported C keymap escape"),
                };
                bytes.push(value);
            }
            assert!(
                !bytes.contains(&0),
                "C key mappings must not contain embedded NUL"
            );
            result.push(Token::Bytes(bytes));
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            i += 1;
            while i < input.len() && (input[i].is_ascii_alphanumeric() || input[i] == b'_') {
                i += 1;
            }
            result.push(Token::Word(
                std::str::from_utf8(&input[start..i]).unwrap().to_owned(),
            ));
            continue;
        }
        if c.is_ascii_digit() || c == b'+' || c == b'-' {
            let start = i;
            i += 1;
            while i < input.len() && input[i].is_ascii_digit() {
                i += 1;
            }
            result.push(Token::Number(
                std::str::from_utf8(&input[start..i])
                    .unwrap()
                    .parse()
                    .unwrap(),
            ));
            continue;
        }
        assert!(b"{}(),|".contains(&c), "unsupported keymap punctuation");
        result.push(Token::Punct(c));
        i += 1;
    }
    result
}
struct Parser {
    tokens: std::vec::IntoIter<Token>,
}
impl Parser {
    fn next(&mut self) -> Token {
        self.tokens.next().expect("incomplete keymap row")
    }
    fn punct(&mut self, value: u8) {
        assert_eq!(self.next(), Token::Punct(value));
    }
    fn word(&mut self) -> String {
        match self.next() {
            Token::Word(value) => value,
            _ => panic!("keymap identifier"),
        }
    }
    fn number(&mut self) -> i32 {
        match self.next() {
            Token::Number(value) => value,
            _ => panic!("keymap mode"),
        }
    }
}
fn main() {
    println!("cargo:rustc-check-cfg=cfg(worminal_gdk_x11)");
    let targets = std::process::Command::new("pkg-config")
        .args(["--variable=targets", "gtk+-3.0"])
        .output()
        .expect("GTK backend configuration");
    assert!(targets.status.success(), "GTK backend configuration");
    if targets
        .stdout
        .split(u8::is_ascii_whitespace)
        .any(|target| target == b"x11")
    {
        println!("cargo:rustc-cfg=worminal_gdk_x11");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let state = root.join("src/gtk/window_state_protocol.h");
    println!("cargo:rerun-if-changed={}", state.display());
    bindgen::Builder::default()
        .header(state.to_str().unwrap())
        .allowlist_var("WORMINAL_STATE_.*")
        .generate()
        .expect("window-state protocol bindings")
        .write_to_file(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("window_state.rs"))
        .expect("write window-state protocol");
    let version = root.join("src/version.h");
    println!("cargo:rerun-if-changed={}", version.display());
    let version = fs::read_to_string(version).unwrap();
    let version = version
        .lines()
        .find_map(|line| {
            line.strip_prefix("#define WORMINAL_VERSION \"")
                .and_then(|value| value.strip_suffix('"'))
        })
        .expect("product version header");
    println!("cargo:rustc-env=WORMINAL_VERSION={version}");
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../src/terminal/keymap.inc");
    println!("cargo:rerun-if-changed={}", path.display());
    let mut parser = Parser {
        tokens: tokens(&fs::read(path).unwrap()).into_iter(),
    };
    let mut output =
        String::from("// Generated from the shared C keymap.\nstatic KEYS: &[KeyEntry] = &[\n");
    while let Some(token) = parser.tokens.next() {
        assert_eq!(token, Token::Punct(b'{'));
        assert_eq!(parser.word(), "KEY");
        parser.punct(b'(');
        let key = parser.word();
        parser.punct(b')');
        parser.punct(b',');
        let mut modifiers = Vec::new();
        loop {
            let modifier = match parser.word().as_str() {
                "KEY_ANY_MOD" => "u32::MAX",
                "KEY_NO_MOD" => "0",
                "KEY_SHIFT" => "gdk::ModifierType::SHIFT_MASK.bits()",
                "KEY_CTRL" => "gdk::ModifierType::CONTROL_MASK.bits()",
                "KEY_ALT" => "gdk::ModifierType::MOD1_MASK.bits()",
                "KEY_SUPER" => "gdk::ModifierType::MOD4_MASK.bits()",
                "KEY_MOD3" => "gdk::ModifierType::MOD3_MASK.bits()",
                _ => panic!("unsupported keymap modifier"),
            };
            modifiers.push(modifier);
            match parser.next() {
                Token::Punct(b'|') => {}
                Token::Punct(b',') => break,
                _ => panic!("keymap mask"),
            }
        }
        let bytes = match parser.next() {
            Token::Bytes(value) => value,
            _ => panic!("keymap bytes"),
        };
        parser.punct(b',');
        let appkey = parser.number();
        parser.punct(b',');
        let appcursor = parser.number();
        parser.punct(b'}');
        parser.punct(b',');
        output.push_str(&format!(
            "KeyEntry {{ key: gdk::keys::constants::{key}, mask: {}, bytes: &{bytes:?}, \
            appkey: {appkey}, appcursor: {appcursor} }},\n",
            modifiers.join(" | ")
        ));
    }
    output.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("keymap.rs"),
        output,
    )
    .unwrap();
}
