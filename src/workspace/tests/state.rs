use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
    },
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};
use worminal_protocol::{Buffer, Decoder, Packet, WIRE_SERVICE_INFO};
use worminal_workspace::{
    browser_target,
    launch::Launch,
    recovery::{self, Recipe, Recovery},
    rights, socket,
};

#[test]
fn descriptor_bundles_are_owned_cloexec_and_closed_after_marker_or_capacity_rejection() {
    use std::{
        fs::File,
        os::fd::{AsFd, AsRawFd},
        os::unix::net::UnixStream,
    };
    let fixture = Fixture::new();
    let path = fixture.root.join("descriptor-file");
    fs::write(&path, b"bundle-bytes").unwrap();
    let file = File::open(&path).unwrap();
    let copies = || {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| fs::read_link(entry.path()).is_ok_and(|target| target == path))
            .count()
    };
    let (sender, receiver) = UnixStream::pair().unwrap();
    assert_eq!(copies(), 1);
    rights::send_descriptors(sender.as_fd(), &[file.as_fd(), file.as_fd()], b'H').unwrap();
    let received = rights::receive_descriptors(receiver.as_fd(), b'H', 2).unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(copies(), 3);
    for descriptor in &received {
        let info =
            fs::read_to_string(format!("/proc/self/fdinfo/{}", descriptor.as_raw_fd())).unwrap();
        let flags = info
            .lines()
            .find_map(|line| line.strip_prefix("flags:\t"))
            .unwrap();
        assert_ne!(
            u32::from_str_radix(flags, 8).unwrap() & libc::O_CLOEXEC as u32,
            0
        );
    }
    let mut received = received.into_iter();
    let mut bytes = Vec::new();
    File::from(received.next().unwrap())
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"bundle-bytes");
    drop(received);
    assert_eq!(copies(), 1);
    rights::send_descriptors(sender.as_fd(), &[file.as_fd()], b'H').unwrap();
    assert!(rights::receive_descriptors(receiver.as_fd(), b'X', 1).is_err());
    assert_eq!(copies(), 1);
    rights::send_descriptors(
        sender.as_fd(),
        &[file.as_fd(), file.as_fd(), file.as_fd()],
        b'H',
    )
    .unwrap();
    assert!(rights::receive_descriptors(receiver.as_fd(), b'H', 1).is_err());
    assert_eq!(copies(), 1);
}

#[test]
fn browser_target_resolution_matches_c_for_urls_hosts_and_unix_path_bytes() {
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let oracle = fixture.project.join("build/browser_target_reference");
    let mut cases: Vec<OsString> = [
        "",
        "about:blank",
        "https://example.invalid/a%20b",
        "worminal:test",
        "localhost",
        "LOCALHOST:8080/a",
        "host.invalid:443/path?q=x#fragment",
        "[::1]:80/a",
        "[::1]",
        "[invalid]:80",
        "host.invalid:bad",
        "host..invalid",
        "-host.invalid",
        "host-.invalid",
        "host.invalid.",
        "./missing.invalid",
        "../missing file",
        "Cargo.toml",
        "missing.invalid",
        "file:///already%20encoded",
        "//missing/folder",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    let path = fixture.root.join("space #\u{e9}");
    fs::write(&path, b"file").unwrap();
    cases.push(path.into_os_string());
    let mut invalid = fixture.root.as_os_str().as_bytes().to_vec();
    invalid.extend_from_slice(b"/missing-\xff");
    cases.push(OsString::from_vec(invalid));
    for case in &cases {
        let result = Command::new(&oracle).arg(case).output().unwrap();
        assert!(result.status.success());
        assert_eq!(
            browser_target::resolve(Some(case)).unwrap(),
            result.stdout,
            "{case:?}"
        );
    }
    let result = Command::new(&oracle).output().unwrap();
    assert!(result.status.success());
    assert_eq!(
        browser_target::resolve(None::<&OsStr>).unwrap(),
        result.stdout
    );
}

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    project: PathBuf,
    scope: String,
}

impl Fixture {
    fn new() -> Self {
        let project = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap();
        let scope = format!(
            "rust-workspace-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        let root = project.join("build/checks").join(&scope);
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            project,
            scope,
        }
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn path(&self) -> PathBuf {
        recovery::path(&self.state(), self.scope.as_bytes()).unwrap()
    }
    fn reference(&self) -> Command {
        let mut command = Command::new(self.project.join("build/recovery_reference"));
        command
            .env("XDG_STATE_HOME", self.state())
            .env("WORMINAL_SHARED_SOCKET_SCOPE", &self.scope);
        command
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn request(environment: &[&[u8]]) -> Vec<u8> {
    let mut output = Buffer::default();
    for value in [80, 24, 1, 1, environment.len() as u32] {
        output.put_u32(value).unwrap();
    }
    for field in [
        b"title".as_slice(),
        b"/directory",
        b"",
        b"-",
        b"42",
        b"/bin/sh",
    ] {
        output.put_string(field).unwrap();
    }
    for field in environment {
        output.put_string(field).unwrap();
    }
    output.into_bytes()
}

#[test]
fn launch_bounds_environment_and_restore_marker() {
    let inherited = Launch::decode(&request(&[])).unwrap();
    assert!(inherited.environment.is_none());
    let explicit = Launch::decode(&request(&[
        b"KEEP=yes",
        b"WORMINAL_RESTORE=old",
        b"ignored",
    ]))
    .unwrap();
    assert_eq!(explicit.arguments[0].as_bytes(), b"/bin/sh");
    let restored = explicit.environment_for_restore();
    assert_eq!(restored.len(), 2);
    assert!(restored
        .iter()
        .any(|(key, value)| key == "WORMINAL_RESTORE" && value == "1"));
    let mut invalid = request(&[]);
    invalid[..4].copy_from_slice(&401u32.to_be_bytes());
    assert!(Launch::decode(&invalid).is_err());
    assert!(Launch::decode(&request(&[])[..10]).is_err());
    let mut invalid = request(&[]);
    invalid.push(0);
    assert!(Launch::decode(&invalid).is_err());
}

#[test]
fn recipes_transfer_both_directions_with_the_c_codec() {
    let fixture = Fixture::new();
    let directory = fixture.root.join("terminal");
    fs::create_dir(&directory).unwrap();
    let recipes = vec![
        Recipe::for_tab(directory.as_os_str().as_bytes(), b"", b"/"),
        Recipe::for_tab(b"", b"about:blank", b"/"),
    ];
    let (mut recovery, initial) = Recovery::open(fixture.path());
    assert!(initial.unwrap().is_empty());
    assert!(!recovery.save(&[]).unwrap());
    assert!(recovery.save(&recipes).unwrap());
    assert!(!recovery.save(&recipes).unwrap());
    assert_eq!(
        fs::metadata(fixture.path()).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let output = fixture.reference().arg("--read").output().unwrap();
    assert!(output.status.success());
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    assert_eq!(
        output.stdout,
        format!(
            "T {}\nB {}\n",
            hex(directory.as_os_str().as_bytes()),
            hex(b"about:blank")
        )
        .as_bytes()
    );
    assert!(fixture
        .reference()
        .arg("--write")
        .arg(&directory)
        .arg("")
        .status()
        .unwrap()
        .success());
    let (_, loaded) = Recovery::open(fixture.path());
    assert_eq!(loaded.unwrap(), &recipes[..1]);
}

#[test]
fn invalid_recipes_and_symlinks_cannot_be_overwritten() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path().parent().unwrap()).unwrap();
    fs::write(fixture.path(), b"damaged recipe").unwrap();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.is_err());
    assert!(!recovery.save(&[Recipe::for_tab(b"/", b"", b"/")]).unwrap());
    assert_eq!(fs::read(fixture.path()).unwrap(), b"damaged recipe");
    fs::remove_file(fixture.path()).unwrap();
    let target = fixture.root.join("preserved");
    fs::write(&target, b"do not follow").unwrap();
    symlink(&target, fixture.path()).unwrap();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.is_err());
    assert!(!recovery.save(&[]).unwrap());
    assert_eq!(fs::read(&target).unwrap(), b"do not follow");
}

#[test]
fn invalid_writes_preserve_the_last_published_recipe() {
    let fixture = Fixture::new();
    let (mut recovery, loaded) = Recovery::open(fixture.path());
    assert!(loaded.unwrap().is_empty());
    recovery.save(&[Recipe::for_tab(b"/", b"", b"/")]).unwrap();
    let before = fs::read(fixture.path()).unwrap();
    assert!(recovery
        .save(&[Recipe {
            directory: b"relative".to_vec(),
            url: Vec::new()
        }])
        .is_err());
    assert_eq!(fs::read(fixture.path()).unwrap(), before);
    let mut truncated = before.clone();
    truncated.push(0);
    assert!(recovery::decode(&truncated).is_err());
    assert!(!fs::read_dir(fixture.path().parent().unwrap())
        .unwrap()
        .any(|entry| entry.unwrap().file_name().as_bytes().ends_with(b".pending")));
}

#[test]
fn c_clients_can_connect_to_a_rust_workspace_endpoint() {
    let fixture = Fixture::new();
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let uid = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let prefix = format!("worminal-workspace-v2-{uid}-");
    let mut longest = fixture.scope.clone();
    longest.extend(std::iter::repeat_n('x', 106 - prefix.len() - longest.len()));
    for scope in [&fixture.scope, &longest] {
        let listener = socket::listen(uid, scope.as_bytes()).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut peer = loop {
                match listener.accept() {
                    Ok((peer, _)) => break peer,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(error) => panic!("private workspace accept failed: {error}"),
                }
            };
            peer.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            peer.set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut header = [0; 16];
            peer.read_exact(&mut header).unwrap();
            let (_, packet) = Decoder::default().push(&header).unwrap();
            assert_eq!(packet.unwrap().kind, WIRE_SERVICE_INFO);
            let reply = Packet {
                kind: WIRE_SERVICE_INFO,
                tab: 0,
                body: 4242u32.to_be_bytes().to_vec(),
            };
            peer.write_all(&reply.encode().unwrap()).unwrap();
        });
        let output = Command::new(fixture.project.join("build/wire_reference"))
            .arg("--socket")
            .env("WORMINAL_SHARED_SOCKET_SCOPE", scope)
            .output()
            .unwrap();
        server.join().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"4242\n");
    }
    // A 107-byte abstract name is legal in Linux but unreachable through legacy snprintf clients.
    longest.push('x');
    assert_eq!(
        socket::listen(uid, longest.as_bytes()).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}
