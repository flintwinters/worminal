//! Audited initial daemon spawn. Only called before GTK creates threads; the daemon outlives its GUI launcher.
use std::{
    io,
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn daemon(path: &Path) -> io::Result<Child> {
    let mut command = Command::new(path);
    command
        .arg("--serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn()
}
