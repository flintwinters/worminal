use crate::{
    process,
    queue::Queue,
    reactor::{self, Interest},
};
use std::{
    ffi::OsString,
    io::{self, Read},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    process::Child,
    thread,
    time::{Duration, Instant},
};
use worminal_protocol::worp::{
    commands::{Command, CommandKind},
    events::EventKind,
    session::Session,
};
use worminal_protocol::*;
use worminal_workspace::{browser_target, socket};

fn connect(scope: &[u8]) -> io::Result<std::os::unix::net::UnixStream> {
    socket::connect(process::uid(), scope)
}

fn ensure(scope: &[u8]) -> io::Result<(std::os::unix::net::UnixStream, Option<Child>)> {
    if let Ok(socket) = connect(scope) {
        return Ok((socket, None));
    }
    let mut command = process::detached_command(&std::env::current_exe()?);
    let mut child = command.arg("--serve").spawn()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(socket) = connect(scope) {
            return Ok((socket, Some(child)));
        }
        if child.try_wait()?.is_some() {
            // Another concurrent launcher may have won the bind race.
            return connect(scope).map(|socket| (socket, None));
        }
        if Instant::now() >= deadline {
            // A failed unpublished startup owns no transferred processes. Do not leave it running.
            child.kill()?;
            child.wait()?;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "workspace service did not become ready",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn bridge(scope: &[u8], existing: bool) -> io::Result<()> {
    let (mut socket, mut child) = if existing {
        (connect(scope)?, None)
    } else {
        ensure(scope)?
    };
    socket.set_nonblocking(true)?;
    let mut input = process::StdioDescriptor::new(0)?;
    let mut output = process::StdioDescriptor::new(1)?;
    let mut incoming = Queue::new(WORKSPACE_IO_BYTES as usize);
    let mut outgoing = Queue::new(WORKSPACE_IO_BYTES as usize);
    let result = (|| loop {
        let interests = [
            Interest {
                fd: 0,
                read: incoming.is_empty(),
                write: false,
            },
            Interest {
                fd: socket.as_raw_fd(),
                read: outgoing.is_empty(),
                write: !incoming.is_empty(),
            },
            Interest {
                fd: 1,
                read: false,
                write: !outgoing.is_empty(),
            },
        ];
        for ready in reactor::wait(&interests, None)? {
            if ready.index == 2 && ready.read && !ready.write {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if ready.index == 0 || (ready.index == 1 && ready.read && outgoing.is_empty()) {
                let mut bytes = [0u8; WORKSPACE_IO_BYTES as usize];
                let count = if ready.index == 0 {
                    input.read(&mut bytes)
                } else {
                    socket.read(&mut bytes)
                };
                match count {
                    Ok(0) => return Ok(()),
                    Ok(count) => {
                        if ready.index == 0 {
                            incoming.append(&bytes[..count])?;
                        } else {
                            outgoing.append(&bytes[..count])?;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => return Err(error),
                }
            }
            if ready.index == 1 && ready.write {
                incoming.flush(&mut socket, WORKSPACE_IO_BYTES as usize)?;
            }
            if ready.index == 2 && ready.write {
                outgoing.flush(&mut output, WORKSPACE_IO_BYTES as usize)?;
            }
        }
    })();
    if let Some(child) = &mut child {
        let _ = child.try_wait();
    }
    result
}

fn request(scope: &[u8], kind: CommandKind) -> io::Result<i32> {
    let socket =
        connect(scope).map_err(|_| io::Error::other("No preview workspace service is running."))?;
    let handoff = matches!(kind, CommandKind::Transfer { .. });
    let timeout = if handoff {
        u64::from(worminal_workspace::handoff::HANDOFF_TIMEOUT_SECONDS)
    } else {
        3
    };
    socket.set_read_timeout(Some(Duration::from_secs(timeout)))?;
    socket.set_write_timeout(Some(Duration::from_secs(timeout)))?;
    let mut session = Session::new(socket.try_clone()?, socket);
    session.send(&Command::new(0, kind))?;
    let event = session.receive()?;
    if matches!(event.kind, EventKind::CommandDone) {
        return Ok(0);
    }
    let error = match event.kind {
        EventKind::Error(bytes) => bytes,
        _ => b"Preview service rejected the command.".to_vec(),
    };
    eprintln!("{}", String::from_utf8_lossy(&error));
    Ok(1)
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    let scope = std::env::var_os("WORMINAL_SHARED_SOCKET_SCOPE").unwrap_or_default();
    let scope = scope.as_os_str().as_bytes();
    if arguments
        .get(1)
        .is_some_and(|value| value == "install-on-remote")
    {
        return crate::remote_install::run(&arguments[2..], false);
    }
    if arguments
        .get(1)
        .is_some_and(|value| value == "upgrade-remote")
    {
        return crate::remote_install::run(&arguments[2..], true);
    }
    if arguments.len() == 2 && arguments[1] == "--version" {
        println!("worminald {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if arguments.get(1).is_some_and(|value| value == "api") {
        return crate::api_cli::run(scope, &arguments[2..]);
    }
    if arguments.len() == 2 && matches!(arguments[1].to_str(), Some("--help" | "-h")) {
        println!(
            "worminald: persistent terminal workspace service\n\
                  Run worminald api help for the automation API.\n\
                  Run worminald install-on-remote --help to bootstrap a new node over SSH.\n\
                  Run worminald upgrade-remote --help to sync an older node with a newer peer.\n\
                  Service commands: --serve, --bridge, --stop, --handin NAME, --handoff NAME, \
                  --new-browser-tab URL|FILE"
        );
        return Ok(0);
    }
    if arguments.len() == 2 {
        match arguments[1].as_os_str().as_bytes() {
            b"--serve" => {
                process::prepare_service()?;
                crate::serve(scope)?;
                return Ok(0);
            }
            b"--upgrade-receiver" | b"--handoff-receiver" => {
                process::prepare_service()?;
                crate::service::receive_upgrade(scope, arguments[1] == "--handoff-receiver")?;
                return Ok(0);
            }
            b"--bridge" | b"--bridge-existing" => {
                bridge(scope, arguments[1] == "--bridge-existing")?;
                return Ok(0);
            }
            b"--stop" => {
                let socket = connect(scope)?;
                Session::new(socket.try_clone()?, socket)
                    .send(&Command::new(0, CommandKind::Stop))?;
                return Ok(0);
            }
            _ => {}
        }
    }
    if arguments.len() == 3 {
        let kind = match arguments[1].as_os_str().as_bytes() {
            b"--handin" => Some(WIRE_HANDIN),
            b"--handoff" => Some(WIRE_HANDOFF),
            b"--new-browser-tab" => Some(WIRE_BROWSER_REPLACE),
            _ => None,
        };
        if let Some(kind) = kind {
            let value = if kind == WIRE_BROWSER_REPLACE {
                process::ignore_hangup();
                browser_target::resolve(Some(&arguments[2]))?
            } else {
                arguments[2].as_os_str().as_bytes().to_vec()
            };
            let command = if kind == WIRE_BROWSER_REPLACE {
                CommandKind::ReplaceBrowser(value)
            } else {
                CommandKind::Transfer {
                    outbound: kind == WIRE_HANDOFF,
                    token: value,
                }
            };
            let result = request(scope, command);
            if kind != WIRE_BROWSER_REPLACE && !matches!(&result, Ok(0)) {
                eprintln!(
                    "Handoff unavailable or rejected; no automatic daemon restart was attempted."
                );
            }
            return result;
        }
    }
    eprintln!(
        "usage: worminald api COMMAND (run 'worminald api help' for commands)\n\
               worminald --serve|--bridge|--stop\n\
               worminald --handin NAME | --handoff NAME\n       \
               worminald --new-browser-tab URL|FILE"
    );
    Ok(2)
}
