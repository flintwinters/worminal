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
    launch::Launch,
    recovery::{self, Recipe, Recovery},
    socket,
};

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
        let root = project.join(".checks").join(&scope);
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
    let listener = socket::listen(uid, fixture.scope.as_bytes()).unwrap();
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
        .env("WORMINAL_SHARED_SOCKET_SCOPE", &fixture.scope)
        .output()
        .unwrap();
    server.join().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"4242\n");
}
