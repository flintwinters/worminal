//! Caller-side diagnostics and stable machine-readable failures. Inspection never starts a service.
use std::{ffi::OsStr, io};
use worminal_protocol::worp::{Reply, Request};

pub(crate) fn failure(code: &str, message: &str, hint: Option<&str>) -> Reply {
    Reply {
        success: false,
        json: serde_json::json!({"ok":false,"error":message,"code":code,"hint":hint}).to_string(),
    }
}

pub(crate) fn io_failure(error: &io::Error) -> Reply {
    let (code, hint) = match error.kind() {
        io::ErrorKind::InvalidInput => ("invalid_arguments", Some("Run worminald api help.")),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound => (
            "unavailable",
            Some("Check the workspace scope and target; inspection never starts a daemon."),
        ),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => (
            "timeout",
            Some(
                "Check connectivity and daemon compatibility; inspect before repeating mutations.",
            ),
        ),
        io::ErrorKind::InvalidData => (
            "incompatible_reply",
            Some("Check daemon versions with api doctor --json."),
        ),
        io::ErrorKind::PermissionDenied => ("permission_denied", None),
        _ => (
            "transport_error",
            Some("Delivery may be uncertain; inspect before repeating mutations."),
        ),
    };
    failure(code, &error.to_string(), hint)
}

pub(super) fn doctor(scope: &[u8], master: Option<&OsStr>) -> Reply {
    let result = super::request(scope, master, Request::List);
    let mut value = match result {
        Ok(reply) => match serde_json::from_str::<serde_json::Value>(&reply.json) {
            Ok(value) => value,
            Err(error) => serde_json::from_str(
                &io_failure(&io::Error::new(io::ErrorKind::InvalidData, error)).json,
            )
            .unwrap(),
        },
        Err(error) => serde_json::from_str(&io_failure(&error).json).unwrap(),
    };
    value["diagnostics"] = serde_json::json!({
        "client_version":env!("CARGO_PKG_VERSION"),
        "scope":String::from_utf8_lossy(scope),
        "target":master.map_or("local".into(), |host| host.to_string_lossy()),
        "socket":if master.is_none() { Some(format!("@{}-{}-{}",
            worminal_protocol::WORKSPACE_ENDPOINT.to_string_lossy(), crate::process::uid(), String::from_utf8_lossy(scope))) } else { None },
        "compatible":value["api_version"].as_u64().map(|v| v == 1),
        "shell_markers":value["tabs"].as_array().map(|tabs| tabs.iter().filter(|t| t["kind"] == "terminal")
            .map(|t| serde_json::json!({"id":t["id"],"available":!t["state"]["shell"]["phase"].is_null()})).collect::<Vec<_>>())
    });
    Reply {
        success: value["ok"] == true,
        json: value.to_string(),
    }
}
