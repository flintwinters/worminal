//! Human CLI presentation only. Wire replies, JSON and runnable source bypass this module.
use serde_json::Value;
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

/// Human layout: a terminal without agent markers. Color additionally honors the opt-outs below.
fn human(
    terminal: bool,
    variables: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
) -> bool {
    terminal
        && !variables
            .into_iter()
            .any(|(name, _)| agent_variable(name.as_ref()))
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

fn scalar(value: &Value, color: bool) -> String {
    let (text, code) = match value {
        Value::String(text) => (text.clone(), ""),
        Value::Bool(true) => ("yes".into(), "32"),
        Value::Bool(false) => ("no".into(), "2"),
        Value::Null => ("-".into(), "2"),
        other => (other.to_string(), ""),
    };
    if color && !code.is_empty() {
        paint(code, &text)
    } else {
        text
    }
}

fn visible(value: &str) -> usize {
    let mut width = 0;
    let mut escape = false;
    for character in value.chars() {
        match (escape, character) {
            (_, '\x1b') => escape = true,
            (true, 'm') => escape = false,
            (false, _) => width += 1,
            _ => {}
        }
    }
    width
}

fn pad(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(visible(value)))
    )
}

fn table(rows: &[Value], color: bool, margin: &str, output: &mut String) {
    let heading = |text: &str| {
        if color {
            paint("1;35", text)
        } else {
            text.into()
        }
    };
    let mut columns: Vec<&str> = Vec::new();
    for key in rows.iter().flat_map(|row| row.as_object().unwrap().keys()) {
        if !columns.contains(&key.as_str()) {
            columns.push(key);
        }
    }
    // JSON keys arrive alphabetized: lead with the identifying text columns, keep the rest in order.
    columns.sort_by_key(|key| {
        !rows
            .iter()
            .any(|row| row.get(*key).is_some_and(Value::is_string))
    });
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|key| match row.get(*key) {
                    Some(Value::Array(items))
                        if items.iter().all(|i| !i.is_object() && !i.is_array()) =>
                    {
                        items
                            .iter()
                            .map(|i| scalar(i, false))
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                    Some(cell) => scalar(cell, color),
                    None => scalar(&Value::Null, color),
                })
                .collect()
        })
        .collect();
    let widths: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            cells
                .iter()
                .map(|row| visible(&row[i]))
                .chain([name.len()])
                .max()
                .unwrap()
        })
        .collect();
    let line = |row: Vec<String>| {
        let joined: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| pad(cell, *width))
            .collect();
        format!("{margin}{}\n", joined.join("  ").trim_end())
    };
    output.push_str(&line(
        columns
            .iter()
            .map(|name| heading(&name.to_uppercase()))
            .collect(),
    ));
    for row in cells {
        output.push_str(&line(row));
    }
}

/// Lay out any JSON reply for people: uniform arrays of objects become tables, objects become
/// aligned key/value lists, and failures become one error line. Commands never format by hand.
fn layout(value: &Value, color: bool, indent: usize, output: &mut String) {
    let margin = " ".repeat(indent);
    let heading = |text: &str| {
        if color {
            paint("1;35", text)
        } else {
            text.into()
        }
    };
    match value {
        Value::Array(rows) if rows.is_empty() => {
            output.push_str(&format!("{margin}{}\n", scalar(&Value::Null, color)));
        }
        Value::Array(rows) if rows.iter().all(Value::is_object) => {
            table(rows, color, &margin, output);
        }
        Value::Array(items) => {
            for item in items {
                output.push_str(&format!("{margin}- {}\n", scalar(item, color)));
            }
        }
        Value::Object(fields) => {
            let width = fields.keys().map(String::len).max().unwrap_or(0);
            for (key, field) in fields {
                let label = pad(&heading(key), width);
                if matches!(field, Value::Object(_) | Value::Array(_))
                    && !field.as_array().is_some_and(|a| a.is_empty())
                {
                    output.push_str(&format!("{margin}{}\n", heading(&format!("{key}:"))));
                    layout(field, color, indent + 2, output);
                } else if field.as_array().is_some_and(|a| a.is_empty()) {
                    output.push_str(&format!(
                        "{margin}{label}  {}\n",
                        scalar(&Value::Null, color)
                    ));
                } else {
                    output.push_str(&format!("{margin}{label}  {}\n", scalar(field, color)));
                }
            }
        }
        other => output.push_str(&format!("{margin}{}\n", scalar(other, color))),
    }
}

/// Render a command's JSON reply: the exact compact JSON for agents, pipes and redirection, or a
/// readable layout on a human terminal. A reply with `masters` etc. needs no per-command renderer.
pub fn reply(value: &Value) -> String {
    let variables: Vec<_> = std::env::vars_os().collect();
    let terminal = std::io::stdout().is_terminal();
    if !human(terminal, variables.iter().cloned()) {
        return format!("{value}\n");
    }
    let color = enabled(terminal, variables);
    let mut output = String::new();
    let failed = value.get("ok") == Some(&Value::Bool(false));
    let mut body = value.clone();
    if let Some(fields) = body.as_object_mut() {
        if fields.get("ok") == Some(&Value::Bool(true)) {
            fields.remove("ok");
        }
        if failed {
            if let Some(Value::String(error)) = fields.remove("error") {
                fields.remove("ok");
                let line = format!("error: {error}");
                output.push_str(&format!(
                    "{}\n",
                    if color { paint("31", &line) } else { line }
                ));
            }
        }
    }
    let plain_empty = body.as_object().is_some_and(|fields| fields.is_empty());
    if !plain_empty {
        layout(&body, color, 0, &mut output);
    }
    if output.is_empty() {
        output.push_str(&format!(
            "{}\n",
            if color {
                paint("32", "ok")
            } else {
                "ok".into()
            }
        ));
    }
    output
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

    #[test]
    fn replies_lay_out_tables_and_objects_generically() {
        let mut output = String::new();
        let value = serde_json::json!([
            {"endpoint": "user@host", "connected": true},
            {"endpoint": "a", "connected": false}
        ]);
        layout(&value, false, 0, &mut output);
        assert_eq!(
            output,
            "ENDPOINT   CONNECTED\nuser@host  yes\na          no\n"
        );
        let mut output = String::new();
        layout(
            &serde_json::json!({"endpoint": "e", "masters": []}),
            false,
            0,
            &mut output,
        );
        assert_eq!(output, "endpoint  e\nmasters   -\n");
    }

    #[test]
    fn replies_are_compact_json_off_human_terminals() {
        // Test processes have no terminal stdout.
        assert_eq!(reply(&serde_json::json!({"ok": true})), "{\"ok\":true}\n");
    }
}
