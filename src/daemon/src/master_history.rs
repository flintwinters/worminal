//! Passive CLI discovery and successful outbound view connection accounting.
use serde_json::json;
use std::{ffi::OsString, io};
use worminal_protocol::{
    worp::{events::EventKind, v2_events},
    Decoder, WIRE_CATALOG,
};
use worminal_workspace::{
    cli_output,
    masters::{Connection, Registry},
};

const HELP: &str = "Usage: worminald --list-masters [--help]
List this user's currently and previously connected master SSH routes as JSON.

Reply: {\"ok\":true,\"masters\":[{\"endpoint\":\"user@host\",\"connected\":true}]}
Entries are sorted by exact SSH endpoint; aliases are not merged. connected is true while
at least one view connection that received a valid master catalog remains open.
History starts with this feature, persists after disconnect/restart, and spans local
workspace scopes and displays sharing XDG_STATE_HOME (default ~/.local/state).
Records live under XDG_STATE_HOME/worminal/masters. Failed connection attempts, explicit
configuration sync and enrolled devices are not connection history.
Recording errors are logged to syslog and do not disconnect views.
Listing is passive: it never starts a daemon, opens SSH, probes a host or takes control.
";

fn registry() -> Registry {
    Registry::new(worminal_config::state_home().join("worminal/masters"))
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    if arguments.len() == 1 && matches!(arguments[0].to_str(), Some("--help" | "-h")) {
        print!("{}", cli_output::text(HELP));
        return Ok(0);
    }
    if !arguments.is_empty() {
        println!(
            "{}",
            json!({"ok": false, "error": "Run worminald --list-masters --help for usage."})
        );
        return Ok(2);
    }
    let result = registry().list().map(|masters| {
        json!({"ok": true, "masters": masters.into_iter().map(|master| {
            json!({"endpoint": master.endpoint, "connected": master.connected})
        }).collect::<Vec<_>>()})
    });
    match result {
        Ok(value) => {
            println!("{value}");
            Ok(0)
        }
        Err(error) => {
            println!("{}", json!({"ok": false, "error": error.to_string()}));
            Ok(1)
        }
    }
}

pub struct Observer {
    endpoint: String,
    decoder: Option<Decoder>,
    _connection: Option<Connection>,
}

impl Observer {
    pub fn new(endpoint: &str) -> Self {
        Self {
            endpoint: endpoint.to_owned(),
            decoder: Some(Decoder::default()),
            _connection: None,
        }
    }

    pub fn observe(&mut self, mut bytes: &[u8]) {
        let Some(decoder) = &mut self.decoder else {
            return;
        };
        let catalog = (|| -> io::Result<bool> {
            while !bytes.is_empty() {
                let (used, packet) = decoder.push(bytes)?;
                bytes = &bytes[used..];
                if let Some(packet) = packet.filter(|packet| packet.kind == WIRE_CATALOG) {
                    return Ok(v2_events::decode(packet)?
                        .is_some_and(|event| matches!(event.kind, EventKind::Catalog(_))));
                }
            }
            Ok(false)
        })();
        if matches!(catalog, Ok(false)) {
            return;
        }
        // A valid catalog is the connection commit point. Upgrades/authentication alone do not
        // prove a workspace connection. Stop decoding immediately; the relay forwards every byte unchanged.
        self.decoder = None;
        let result = catalog.and_then(|_| registry().connected(&self.endpoint));
        match result {
            Ok(connection) => self._connection = Some(connection),
            Err(error) => {
                crate::process::diagnostic(&format!("Master history unavailable: {error}"))
            }
        }
    }
}
