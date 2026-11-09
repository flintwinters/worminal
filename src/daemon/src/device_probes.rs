//! Caller-side observations: network waits never run in the terminal service loop.
use serde_json::Value;
use std::{
    io, thread,
    time::{SystemTime, UNIX_EPOCH},
};
use worminal_protocol::worp::{
    devices::{Device, MAX_DEVICES},
    Reply,
};

const PARALLEL_PROBES: usize = 4;

pub fn validate(value: &Value) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "Invalid device registry reply.");
    let devices = value
        .get("devices")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    if devices.len() > MAX_DEVICES {
        return Err(invalid());
    }
    for value in devices {
        let device = Device {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .to_owned(),
            endpoint: value
                .get("endpoint")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?
                .to_owned(),
        };
        device.validate()?;
    }
    Ok(())
}

pub fn check(
    value: &mut Value,
    probe: impl Fn(&str) -> io::Result<Reply> + Sync,
) -> io::Result<()> {
    validate(value)?;
    let devices = value
        .get_mut("devices")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Invalid registry reply."))?;
    for batch in devices.chunks_mut(PARALLEL_PROBES) {
        thread::scope(|scope| {
            for device in batch {
                let probe = &probe;
                scope.spawn(move || {
                    let result = device
                        .get("endpoint")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "Invalid device endpoint.")
                        })
                        .and_then(probe);
                    let failure = match result {
                        Ok(reply) if reply.success => {
                            let valid = serde_json::from_str::<Value>(&reply.json)
                                .ok()
                                .is_some_and(|value| {
                                    value.get("ok").and_then(Value::as_bool) == Some(true)
                                        && value.get("tabs").is_some_and(Value::is_array)
                                });
                            (!valid).then(|| "Invalid Worp listing reply.".to_owned())
                        }
                        Ok(reply) => Some(
                            serde_json::from_str::<Value>(&reply.json)
                                .ok()
                                .and_then(|value| {
                                    value
                                        .get("error")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .unwrap_or_else(|| "Worp listing was rejected.".to_owned()),
                        ),
                        Err(error) => Some(error.to_string()),
                    };
                    device["availability"] = Value::from(if failure.is_some() {
                        "unavailable"
                    } else {
                        "reachable"
                    });
                    if let Some(error) = failure {
                        device["detail"] = Value::from(error.chars().take(512).collect::<String>());
                    }
                    let checked = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .and_then(|duration| u64::try_from(duration.as_millis()).ok());
                    device["checked_at_unix_ms"] = serde_json::json!(checked);
                });
            }
        });
    }
    value["availability_vantage"] = Value::from("caller");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn observations_preserve_membership_and_bound_parallel_work() {
        let mut value = serde_json::json!({"devices": (0..9).map(|n| serde_json::json!({
            "name":format!("node{n}"),"endpoint":format!("host{n}"),"availability":"unchecked"
        })).collect::<Vec<_>>()});
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let calls = AtomicUsize::new(0);
        check(&mut value, |endpoint| {
            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(count, Ordering::SeqCst);
            calls.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(2));
            active.fetch_sub(1, Ordering::SeqCst);
            if endpoint == "host1" {
                return Err(io::Error::other("authentication failed"));
            }
            Ok(Reply {
                success: true,
                json: "{\"ok\":true,\"tabs\":[]}".into(),
            })
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 9);
        assert!(peak.load(Ordering::SeqCst) <= PARALLEL_PROBES);
        assert_eq!(value["devices"].as_array().unwrap().len(), 9);
        assert_eq!(value["devices"][0]["availability"], "reachable");
        assert_eq!(value["devices"][1]["availability"], "unavailable");
        assert_eq!(value["devices"][1]["endpoint"], "host1");
        assert_eq!(value["devices"][1]["detail"], "authentication failed");
        assert_eq!(value["availability_vantage"], "caller");
    }

    #[test]
    fn invalid_or_unbounded_remote_registry_causes_no_probes() {
        for mut value in [
            serde_json::json!({"devices":[null]}),
            serde_json::json!({"devices":vec![serde_json::json!({"name":"node","endpoint":"host"}); MAX_DEVICES+1]}),
            serde_json::json!([1, 2]),
        ] {
            assert!(check(&mut value, |_| panic!(
                "invalid registry must not initiate network activity"
            ))
            .is_err());
        }
    }
}
