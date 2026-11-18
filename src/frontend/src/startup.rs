//! Detached helpers (the local daemon, link openers) start through the shared GUI-safe spawn
//! boundary: own session, /dev/null stdio (version sync keeps stdout for its receipt), and a reaper thread.
use std::{
    ffi::{CString, OsStr, OsString},
    fs::OpenOptions,
    io::{self, Read},
    os::{fd::AsFd, unix::ffi::OsStrExt},
    path::Path,
    sync::{
        mpsc::{self, Receiver},
        Mutex,
    },
    thread,
};

pub fn daemon(path: &Path) -> io::Result<Receiver<io::Result<i32>>> {
    detached(&[
        CString::new(path.as_os_str().as_bytes())?,
        CString::new("--serve").unwrap(),
    ])
}
pub fn open_url(url: &str) -> io::Result<()> {
    detached(&[CString::new("xdg-open").unwrap(), CString::new(url)?]).map(drop)
}
fn detached(arguments: &[CString]) -> io::Result<Receiver<io::Result<i32>>> {
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let pid = worminal_workspace::spawn::start(
        arguments,
        &[(null.as_fd(), 0), (null.as_fd(), 1), (null.as_fd(), 2)],
        true,
    )?;
    let (sender, receiver) = mpsc::sync_channel(1);
    // Never kill a successful spawn if waiter setup fails: a daemon may already own restored shells.
    thread::Builder::new()
        .name("worminal-reap".into())
        .spawn(move || {
            let _ = sender.send(worminal_workspace::spawn::wait(pid));
        })?;
    Ok(receiver)
}

pub fn replacement(executable: &Path, token: &str) -> io::Result<worminal_workspace::spawn::Child> {
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let pid = worminal_workspace::spawn::start(
        &[
            CString::new(executable.as_os_str().as_bytes())?,
            CString::new("--replace-owner").unwrap(),
            CString::new(token)?,
        ],
        &[(null.as_fd(), 0), (null.as_fd(), 1), (null.as_fd(), 2)],
        true,
    )?;
    worminal_workspace::spawn::own(pid, "worminal-ui-replacement")
}

/// Sync the daemon versions of this machine and a connected master, once per host per owner.
/// The helper runs `worminald upgrade-remote`, which updates whichever side is older and logs the outcome.
/// When it fails, `failed` receives its error on the default main context.
pub fn sync_versions(
    master: &OsStr,
    failed: impl FnOnce(String) + Send + 'static,
) -> io::Result<()> {
    static CHECKED: Mutex<Vec<OsString>> = Mutex::new(Vec::new());
    let mut checked = CHECKED.lock().unwrap();
    if checked.iter().any(|host| host == master) {
        return Ok(());
    }
    checked.push(master.to_owned());
    let daemon = std::env::current_exe()?.with_file_name("worminald");
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")?;
    let (mut receipt, output) = io::pipe()?;
    let pid = worminal_workspace::spawn::start(
        &[
            CString::new(daemon.as_os_str().as_bytes())?,
            CString::new("upgrade-remote").unwrap(),
            CString::new(master.as_bytes())?,
        ],
        &[(null.as_fd(), 0), (output.as_fd(), 1), (null.as_fd(), 2)],
        true,
    )?;
    drop(output);
    thread::Builder::new()
        .name("worminal-sync".into())
        .spawn(move || {
            // Drain everything so the helper never blocks on a full pipe; keep a bounded prefix.
            let mut text = Vec::new();
            let _ = (&mut receipt).take(64 * 1024).read_to_end(&mut text);
            let _ = io::copy(&mut receipt, &mut io::sink());
            if let Some(error) = sync_failure(worminal_workspace::spawn::wait(pid), &text) {
                glib::MainContext::default().invoke(move || failed(error));
            }
        })?;
    Ok(())
}

/// The error of a failed `upgrade-remote` run, from its JSON receipt when it wrote one.
fn sync_failure(status: io::Result<i32>, receipt: &[u8]) -> Option<String> {
    if matches!(status, Ok(0)) {
        return None;
    }
    let error = serde_json::from_slice::<serde_json::Value>(receipt)
        .ok()
        .and_then(|receipt| receipt.get("error")?.as_str().map(str::to_owned));
    Some(
        error.unwrap_or_else(|| {
            "the sync helper failed without a report; see the system log".into()
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::sync_failure;
    use std::io;

    #[test]
    fn version_sync_reports_only_failed_helpers() {
        assert_eq!(sync_failure(Ok(0), br#"{"ok":true}"#), None);
        assert_eq!(
            sync_failure(Ok(1 << 8), br#"{"ok":false,"error":"cannot run here"}"#).as_deref(),
            Some("cannot run here")
        );
        // A crashed or unreadable helper still alerts, without a receipt.
        assert!(sync_failure(Ok(9), b"").is_some());
        assert!(sync_failure(Err(io::Error::other("lost")), b"").is_some());
    }
}
