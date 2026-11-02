//! Dedicated process: its fork hooks cannot affect unrelated workspace fixtures.
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use worminal_workspace::ssh;

static FORKS: AtomicUsize = AtomicUsize::new(0);
unsafe extern "C" fn fork_hook() {
    FORKS.fetch_add(1, Ordering::Relaxed);
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn ssh_preserves_bridge_arguments_and_bidirectional_bytes_without_gui_fork_hooks() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let fixture = Fixture(root.join(format!("build/checks/rust-ssh-{}", std::process::id())));
    fs::create_dir_all(&fixture.0).unwrap();
    let program = fixture.0.join("ssh");
    fs::write(
        &program,
        b"#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$WORMINAL_TEST_SSH_ARGUMENTS\"\nexec /bin/cat\n",
    )
    .unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    let arguments = fixture.0.join("arguments");
    let mut path = fixture.0.as_os_str().to_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    std::env::set_var("PATH", path);
    std::env::set_var("WORMINAL_TEST_SSH_ARGUMENTS", &arguments);
    assert_eq!(
        unsafe { libc::pthread_atfork(Some(fork_hook), None, None) },
        0
    );
    for invalid in ["", "-host", "bad\nhost"] {
        assert!(ssh::connect(OsStr::new(invalid)).is_err());
    }
    let (mut stream, child) = ssh::connect(OsStr::new("user@private")).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let input = b"bidirectional\0\x1b[200~\xc3\xa9";
    stream.write_all(input).unwrap();
    let mut output = vec![0; input.len()];
    stream.read_exact(&mut output).unwrap();
    assert_eq!(output, input);
    assert_eq!(
        fs::read_to_string(arguments).unwrap(),
        "-T\n-C\n-oConnectTimeout=10\nuser@private\nworminald\n--bridge\n"
    );
    assert_eq!(
        FORKS.load(Ordering::Relaxed),
        0,
        "SSH invoked GUI-process fork hooks"
    );
    drop(stream);
    drop(child);
    // The local service uses the same boundary after GTK starts; it must have an independent
    // session/process group without invoking the GUI's registered fork hooks.
    use std::{ffi::CString, os::fd::AsFd, os::unix::net::UnixStream};
    let (mut parent, remote) = UnixStream::pair().unwrap();
    parent
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let arguments = [
        "/usr/bin/python3",
        "-I",
        "-c",
        "import os; print(os.getpid(),os.getsid(0),os.getpgrp())",
    ]
    .into_iter()
    .map(|value| CString::new(value).unwrap())
    .collect::<Vec<_>>();
    let pid = worminal_workspace::spawn::start(
        &arguments,
        &[
            (remote.as_fd(), 0),
            (remote.as_fd(), 1),
            (remote.as_fd(), 2),
        ],
        true,
    )
    .unwrap();
    drop(remote);
    let mut identity = String::new();
    parent.read_to_string(&mut identity).unwrap();
    let ids: Vec<i32> = identity
        .split_whitespace()
        .map(|value| value.parse().unwrap())
        .collect();
    assert_eq!(ids, [pid, pid, pid]);
    assert_eq!(worminal_workspace::spawn::wait(pid).unwrap(), 0);
    assert_eq!(
        FORKS.load(Ordering::Relaxed),
        0,
        "daemon startup invoked GUI fork hooks"
    );
    assert_eq!(
        worminal_workspace::spawn::wait(-1).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}
