//! Merge `worminal.yaml` with a peer over SSH stdio. The initiator holds the base and decides; the peer
//! answers with its text and applies the result only if its file is still what it reported.
use serde_json::{json, Value};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    time::Duration,
};
use worminal_config::sync::{self, Prefer};
use worminal_protocol::worp::devices::validate_endpoint;
use worminal_workspace::ssh;

const LIMIT: u64 = 1024 * 1024;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn line(reader: &mut impl BufRead) -> io::Result<Value> {
    let mut text = String::new();
    reader.take(LIMIT).read_line(&mut text)?;
    if !text.ends_with('\n') {
        return Err(invalid(
            "Config peer did not answer; it may predate config sync.",
        ));
    }
    let value: Value =
        serde_json::from_str(&text).map_err(|_| invalid("Invalid config peer response."))?;
    match value.get("error").and_then(Value::as_str) {
        Some(error) => Err(invalid(format!("Config peer refused: {error}"))),
        None => Ok(value),
    }
}

fn text<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Malformed config exchange."))
}

/// Peer side: report the file, then apply one compare-and-set.
pub fn exchange() -> io::Result<()> {
    let path = sync::target_path();
    let current = sync::read_text(&path).map_err(invalid)?;
    let mut out = io::stdout().lock();
    writeln!(out, "{}", json!({"config": current}))?;
    out.flush()?;
    let request = line(&mut io::stdin().lock())?;
    let reply = (|| -> Result<(), String> {
        let Some(new) = request.get("config").and_then(Value::as_str) else {
            return Ok(());
        };
        if request.get("expect").and_then(Value::as_str) != Some(current.as_str())
            || sync::read_text(&path)? != current
        {
            return Err("configuration changed during sync; try again".into());
        }
        let parsed = as_value(new)?;
        worminal_config::Settings::resolve(&parsed)?;
        sync::write_atomic(&path, new)
    })();
    match reply {
        Ok(()) => writeln!(out, "{}", json!({"ok": true}))?,
        Err(error) => writeln!(out, "{}", json!({"error": error}))?,
    }
    out.flush()
}

fn as_value(text: &str) -> Result<Value, String> {
    Ok(sync::unflatten(&sync::parse(text)?))
}

/// Initiator side. Returns a JSON report; conflicts are data, not failure.
pub fn run(endpoint: &str, prefer: Prefer) -> io::Result<Value> {
    validate_endpoint(endpoint)?;
    let (stream, _child) = ssh::connect_config(endpoint.as_ref())?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let remote_text = text(&line(&mut reader)?, "config")?.to_owned();
    let path = sync::target_path();
    let local_text = sync::read_text(&path).map_err(invalid)?;
    let base_path = sync::base_path(endpoint);
    let merged = sync::merge3(
        &sync::parse(&sync::read_text(&base_path).map_err(invalid)?).map_err(invalid)?,
        &sync::parse(&local_text).map_err(invalid)?,
        &sync::parse(&remote_text).map_err(invalid)?,
        prefer,
    );
    let (new_local, new_remote) = (sync::render(&merged.local), sync::render(&merged.remote));
    // The comparison is on canonical text so an unchanged but differently formatted file is not rewritten.
    let canonical = |text: &str| sync::parse(text).map(|flat| sync::render(&flat));
    let changed_remote = canonical(&remote_text).map_err(invalid)? != new_remote;
    let changed_local = canonical(&local_text).map_err(invalid)? != new_local;
    let request = if changed_remote {
        json!({"expect": remote_text, "config": new_remote})
    } else {
        json!({"expect": remote_text})
    };
    writeln!(writer, "{request}")?;
    writer.flush()?;
    line(&mut reader)?; // the peer applied it, or refused and nothing below runs
                        // Local last: a refused peer leaves this node unchanged, and the base only advances after both sides did.
    if changed_local {
        worminal_config::Settings::resolve(&as_value(&new_local).map_err(invalid)?)
            .map_err(invalid)?;
        sync::write_atomic(&path, &new_local).map_err(invalid)?;
    }
    sync::write_atomic(&base_path, &sync::render(&merged.base)).map_err(invalid)?;
    Ok(
        json!({"ok": true, "endpoint": endpoint, "changed_local": changed_local,
              "changed_remote": changed_remote, "conflicts": merged.conflicts}),
    )
}
