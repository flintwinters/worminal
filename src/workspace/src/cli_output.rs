//! Human CLI presentation only. Wire replies, JSON and runnable source bypass this module.
use std::{ffi::OsStr, fmt::Display, io::IsTerminal};

pub const HELP: &str =
    "Color: human terminal output only; agent environment markers, NO_COLOR, CLICOLOR=0\n\
or TERM=dumb disable it. JSON, protocol streams and runnable source remain plain.\n";

fn agent_variable(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    [
        b"CODEX_".as_slice(),
        b"CLAUDE_CODE_",
        b"AIDER_",
        b"OPENCODE_",
        b"GEMINI_CLI_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
        || [
            b"AGENT".as_slice(),
            b"AGENT_MODE",
            b"AI_AGENT",
            b"CLAUDECODE",
            b"CURSOR_AGENT",
            b"AIDER",
            b"OPENCODE",
            b"GEMINI_CLI",
            b"CLINE_ACTIVE",
            b"ROO_CODE",
        ]
        .contains(&name)
}

fn enabled(
    terminal: bool,
    variables: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
) -> bool {
    terminal
        && !variables.into_iter().any(|(name, value)| {
            let name = name.as_ref();
            agent_variable(name)
                || name == "NO_COLOR"
                || (name == "TERM" && value.as_ref() == "dumb")
                || (name == "CLICOLOR" && value.as_ref() == "0")
        })
}

fn paint(code: &str, value: &str) -> String {
    format!("\x1b[{code}m{value}\x1b[0m")
}

fn help(value: &str) -> String {
    let mut output = String::new();
    for (index, line) in value.split_inclusive('\n').enumerate() {
        let body = line.trim_end_matches('\n');
        if !body.is_empty() && (index == 0 || (!body.starts_with(' ') && body.ends_with(':'))) {
            output.push_str(&paint(if index == 0 { "1;36" } else { "1;35" }, body));
            output.push_str(&line[body.len()..]);
            continue;
        }
        for token in line.split_inclusive(char::is_whitespace) {
            let word = token.trim_end_matches(char::is_whitespace);
            let bare = word.trim_matches(['[', ']', '(', ')', '`', ',', ';']);
            let code = if bare.starts_with("worminal") || word.starts_with('`') {
                Some("36")
            } else if bare.starts_with('-') || bare.starts_with("Ctrl+") {
                Some("32")
            } else if bare.chars().any(|c| c.is_ascii_uppercase())
                && !bare.chars().any(|c| c.is_ascii_lowercase())
            {
                Some("33")
            } else {
                None
            };
            if let Some(code) = code {
                output.push_str(&paint(code, word));
            } else {
                output.push_str(word);
            }
            output.push_str(&token[word.len()..]);
        }
    }
    output
}

pub fn text(value: impl Display) -> String {
    let value = value.to_string();
    if enabled(std::io::stdout().is_terminal(), std::env::vars_os()) {
        help(&value)
    } else {
        value
    }
}

pub fn error(value: impl Display) -> String {
    let value = value.to_string();
    if enabled(std::io::stderr().is_terminal(), std::env::vars_os()) {
        paint("31", &value)
    } else {
        value
    }
}

pub fn success(value: impl Display) -> String {
    let value = value.to_string();
    if enabled(std::io::stdout().is_terminal(), std::env::vars_os()) {
        paint("32", &value)
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_requires_a_terminal_without_agent_or_no_color_markers() {
        assert!(enabled(true, [("TERM", "xterm-256color")]));
        assert!(!enabled(false, [("TERM", "xterm-256color")]));
        for name in [
            "CODEX_THREAD_ID",
            "CODEX_SANDBOX",
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CURSOR_AGENT",
            "AIDER_MODEL",
            "OPENCODE",
            "GEMINI_CLI",
            "CLINE_ACTIVE",
            "ROO_CODE",
            "AGENT",
            "AGENT_MODE",
            "AI_AGENT",
            "NO_COLOR",
        ] {
            assert!(!enabled(true, [(name, "")]), "{name}");
        }
        assert!(!enabled(true, [("TERM", "dumb")]));
        assert!(!enabled(true, [("CLICOLOR", "0")]));
        assert!(enabled(
            true,
            [("OPENAI_API_KEY", "configured"), ("SSH_AUTH_SOCK", "agent")]
        ));
    }

    #[test]
    fn styling_preserves_help_text_and_whitespace() {
        let source = "worminald: service\n\nTopics:\n  worminald api help --search QUERY\n  café\n";
        let colored = help(source);
        assert!(colored.contains("\x1b[1;36m"));
        assert!(colored.contains("\x1b[32m--search"));
        let mut plain = String::new();
        let mut escape = false;
        for character in colored.chars() {
            if character == '\x1b' {
                escape = true;
            } else if escape {
                if character == 'm' {
                    escape = false;
                }
            } else {
                plain.push(character);
            }
        }
        assert_eq!(plain, source);
    }
}
