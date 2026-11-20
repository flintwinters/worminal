//! Authenticate and synchronize before allowing a view to reach a remote workspace.
use crate::{config_sync, remote_install};
use serde_json::json;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, BufReader, Write},
    os::{
        fd::{FromRawFd, RawFd},
        unix::process::CommandExt,
    },
    process::Command,
};
use worminal_config::sync::Prefer;
use worminal_protocol::worp::devices::validate_endpoint;
use worminal_workspace::ssh;

/// The bootstrap is the first remote command after SSH authentication. It cannot open a workspace.
/// Subsequent phases use that same connection, including password-only hosts without SSH multiplexing.
fn code() -> String {
    format!(
        "{}\n\
request=header(sys.stdin.buffer.raw)\n\
if request != {{'config': True}}: raise SystemExit(1)\n\
status=subprocess.call([str(PREFIX/'bin/worminald'), 'config', 'exchange'])\n\
message({{'config_done': status == 0}})\n\
request=header(sys.stdin.buffer.raw)\n\
bridge=request.get('bridge')\n\
if bridge not in ('--bridge', '--bridge-existing'): raise SystemExit(1)\n\
os.execv(str(PREFIX/'bin/worminald'), ['worminald', bridge])\n",
        remote_install::code("upgrade")
    )
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    let mut report = report_channel();
    let result = connect(arguments, &mut report);
    if let Err(error) = &result {
        crate::process::diagnostic(&format!("Master connection failed: {error}"));
    }
    result
}

/// Take the view's report pipe (ssh::CONNECT_REPORT_FD) before spawning SSH. Close-on-exec keeps
/// it out of SSH, so the view sees only this helper's report and SSH output can never fill it.
fn report_channel() -> Option<File> {
    let fd: RawFd = std::env::var(ssh::CONNECT_REPORT_ENV).ok()?.parse().ok()?;
    if fd <= 2 || unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return None;
    }
    // SAFETY: the variable names a descriptor the view opened for exactly this helper.
    Some(unsafe { File::from_raw_fd(fd) })
}

fn connect(arguments: &[OsString], report: &mut Option<File>) -> io::Result<i32> {
    let [endpoint, bridge] = arguments else {
        return Err(io::Error::other("Expected master and bridge mode."));
    };
    if !matches!(
        bridge
            .to_str()
            .and_then(worminal_workspace::cli_schema::Daemon::parse),
        Some(
            worminal_workspace::cli_schema::Daemon::Bridge
                | worminal_workspace::cli_schema::Daemon::BridgeExisting
        )
    ) {
        return Err(io::Error::other("Invalid master bridge mode."));
    }
    let endpoint = endpoint
        .to_str()
        .ok_or_else(|| io::Error::other("SSH endpoint must be UTF-8."))?;
    validate_endpoint(endpoint)?;
    let executable = std::env::current_exe()?;
    let (mut stream, _child) =
        ssh::connect_python_guarded(endpoint.as_ref(), &code(), executable.as_os_str())?;
    // A failed version sync closes the link before any workspace access. Tell the view why: it
    // otherwise sees only a closed stream and would retry silently (glibc mismatches did this).
    let mut result = remote_install::upgrade_stream(&mut stream).inspect_err(|error| {
        if let Some(report) = report {
            let _ = writeln!(report, "Version sync with {endpoint} failed: {error}");
        }
    })?;
    result["endpoint"] = json!(endpoint);
    if result.get("current").is_none() {
        crate::process::diagnostic(&format!("Remote upgrade: {result}"));
    }
    // The request/reply boundary prevents either Python or BufReader from consuming bridge bytes.
    // No view input is read or forwarded until both the version and configuration phases finish.
    writeln!(stream, "{}", json!({"config": true}))?;
    stream.flush()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let config = config_sync::exchange_with(endpoint, Prefer::Neither, &mut reader, &mut stream)?;
    if remote_install::receive(&mut reader)?["config_done"] != true {
        return Err(io::Error::other("Remote configuration exchange failed."));
    }
    if config["conflicts"]
        .as_array()
        .is_some_and(|c| !c.is_empty())
    {
        crate::process::diagnostic(&format!("Remote configuration sync: {config}"));
    }
    writeln!(stream, "{}", json!({"bridge": bridge.to_str()}))?;
    stream.flush()?;
    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    let mut history = crate::master_history::Observer::new(endpoint);
    crate::cli::relay_observed(stream, |bytes| history.observe(bytes))?;
    Ok(0)
}

/// posix_spawn has no parent-death action. Set it here before exec, without GUI fork callbacks.
/// Checking the parent after prctl also closes the race where the proxy died before this ran.
pub fn ssh_child(arguments: &[OsString]) -> io::Result<i32> {
    let parent: libc::pid_t = arguments
        .first()
        .and_then(|value| value.to_str())
        .and_then(|value| value.parse().ok())
        .filter(|parent| *parent > 1)
        .ok_or_else(|| io::Error::other("Invalid SSH owner."))?;
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::getppid() != parent {
            return Err(io::Error::other("SSH owner exited before startup."));
        }
    }
    if arguments.get(1).is_none_or(|program| program != "ssh") {
        return Err(io::Error::other("Invalid guarded SSH command."));
    }
    Err(Command::new("ssh").args(&arguments[2..]).exec())
}
