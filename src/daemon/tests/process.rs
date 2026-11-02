use std::{
    fs,
    io::{Read, Write},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
use worminal_daemon::process::{session_of, ChildReaper, Pty};
use worminal_workspace::launch::Launch;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    reaper: ChildReaper,
}
impl Fixture {
    fn new() -> Self {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root = project.join("build/checks").join(format!(
            "rust-pty-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            root: root.canonicalize().unwrap(),
            reaper: ChildReaper::default(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.reaper.reap() != 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn until(pty: &mut Pty, needle: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut output = Vec::new();
    while Instant::now() < deadline {
        let mut bytes = [0; 8192];
        match pty.master().unwrap().read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => {
                output.extend_from_slice(&bytes[..count]);
                if output.windows(needle.len()).any(|s| s == needle) {
                    return output;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("PTY read failed: {error}"),
        }
    }
    panic!(
        "PTY did not produce {:?}; received {:?}",
        String::from_utf8_lossy(needle),
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn child_gets_its_own_session_cwd_environment_and_initial_geometry() {
    let fixture = Fixture::new();
    let mut launch = Launch::restore(fixture.root.as_os_str().as_bytes().to_vec());
    launch.window_id = b"42".to_vec();
    launch.environment = Some(vec![("ONLY_VALUE".into(), "kept".into())]);
    launch.arguments = vec![
        "/bin/sh".into(),
        "-c".into(),
        concat!(
        "printf 'START:%s|%s|%s|%s\\n' \"$PWD\" \"$TERM\" \"$WINDOWID\" \"${ONLY_VALUE-unset}\"; ",
        "/bin/stty size; read -r answer; /bin/stty size; printf 'DONE:%s\\n' \"$answer\""
    )
        .into(),
    ];
    let mut pty = Pty::spawn(&launch, false, fixture.reaper.clone()).unwrap();
    let output = until(&mut pty, b"24 80\r\n");
    assert!(output
        .windows(b"|xterm-256color|42|kept".len())
        .any(|bytes| bytes == b"|xterm-256color|42|kept"));
    let pid = pty.live_pid();
    assert!(pid > 0);
    assert_eq!(session_of(pid).unwrap(), pid);
    pty.resize(100, 30).unwrap();
    pty.master().unwrap().write_all(b"preserved\n").unwrap();
    let output = until(&mut pty, b"DONE:preserved");
    assert!(output
        .windows(b"30 100\r\n".len())
        .any(|s| s == b"30 100\r\n"));
    pty.stop();
}

#[test]
fn real_zsh_restore_marker_is_one_shot_and_preserves_the_saved_directory() {
    let fixture = Fixture::new();
    let default = fixture.root.join("default");
    let saved = fixture.root.join("saved");
    fs::create_dir(&default).unwrap();
    fs::create_dir(&saved).unwrap();
    fs::write(
        fixture.root.join(".zshrc"),
        concat!(
            "[[ -n ${WORMINAL_RESTORE-} ]] || cd -- \"$DEFAULT_DIRECTORY\"\n",
            "print -r -- \"START:$PWD|${WORMINAL_RESTORE-unset}\"\n",
            "unset WORMINAL_RESTORE\nPROMPT=''\nRPROMPT=''\n"
        ),
    )
    .unwrap();
    for restored in [false, true] {
        let mut launch = Launch::restore(saved.as_os_str().as_bytes().to_vec());
        launch.arguments = vec!["/bin/zsh".into(), "-d".into()];
        launch.environment = Some(vec![
            ("SHELL".into(), "/bin/zsh".into()),
            ("ZDOTDIR".into(), fixture.root.clone().into_os_string()),
            ("DEFAULT_DIRECTORY".into(), default.clone().into_os_string()),
        ]);
        let mut pty = Pty::spawn(&launch, restored, fixture.reaper.clone()).unwrap();
        let startup = until(&mut pty, if restored { b"|1\r\n" } else { b"|unset\r\n" });
        let expected = if restored { &saved } else { &default };
        assert!(startup
            .windows(expected.as_os_str().as_bytes().len())
            .any(|s| s == expected.as_os_str().as_bytes()));
        pty.master()
            .unwrap()
            .write_all(b"print -r -- AFTER:${WORMINAL_RESTORE-unset}\n")
            .unwrap();
        until(&mut pty, b"AFTER:unset\r\n");
        pty.stop();
    }
}
