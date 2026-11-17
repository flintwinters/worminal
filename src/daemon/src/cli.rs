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
use worminal_workspace::{browser_target, cli_output, cli_schema::Daemon, socket};

fn connect(scope: &[u8]) -> io::Result<std::os::unix::net::UnixStream> {
    socket::connect(process::uid(), scope)
}

fn ensure(scope: &[u8]) -> io::Result<(std::os::unix::net::UnixStream, Option<Child>)> {
    if let Ok(socket) = connect(scope) {
        return Ok((socket, None));
    }
    let mut command = process::detached_command(&std::env::current_exe()?);
    let mut child = command.arg(Daemon::Serve.name()).spawn()?;
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
    let (socket, mut child) = if existing {
        (connect(scope)?, None)
    } else {
        ensure(scope)?
    };
    let result = relay(socket);
    if let Some(child) = &mut child {
        let _ = child.try_wait();
    }
    result
}

/// Bounded bidirectional transport, shared by local bridges and authenticated remote views.
pub(crate) fn relay(socket: std::os::unix::net::UnixStream) -> io::Result<()> {
    relay_observed(socket, |_| {})
}

pub(crate) fn relay_observed(
    mut socket: std::os::unix::net::UnixStream,
    mut observe: impl FnMut(&[u8]),
) -> io::Result<()> {
    socket.set_nonblocking(true)?;
    let mut input = process::StdioDescriptor::new(0)?;
    let mut output = process::StdioDescriptor::new(1)?;
    let mut incoming = Queue::new(WORKSPACE_IO_BYTES as usize);
    let mut outgoing = Queue::new(WORKSPACE_IO_BYTES as usize);
    loop {
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
                            observe(&bytes[..count]);
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
    }
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
    eprintln!("{}", cli_output::error(String::from_utf8_lossy(&error)));
    Ok(1)
}

/// The same detail providers serve native help and the dynamically enumerated search corpus.
fn detail(command: Daemon) -> Option<String> {
    match command {
        Daemon::Help => Some(crate::api_help::overview()),
        Daemon::Api => Some(format!(
            "{}{}",
            crate::api::help(),
            crate::api_help::index()
        )),
        Daemon::Config => Some(crate::config_cli::reference()),
        Daemon::ListMasters => Some(crate::master_history::HELP.to_owned()),
        Daemon::InstallRemote => Some(crate::remote_install::HELP.to_owned()),
        Daemon::UpgradeRemote => Some(crate::remote_install::UPGRADE_HELP.to_owned()),
        _ => None,
    }
}

pub(crate) fn reference(command: Daemon) -> String {
    detail(command).unwrap_or_else(|| {
        let entry = command.entry();
        format!(
            "worminald {} {}\n{}\n",
            entry.names.join(" | "),
            entry.arguments,
            entry.summary
        )
    })
}

pub(crate) fn help_documents() -> Vec<(String, String)> {
    let mut documents = worminal_workspace::cli_schema::documents();
    documents.push((
        "worminal --help".to_owned(),
        worminal_workspace::cli_help::launcher("worminal", env!("CARGO_PKG_VERSION")),
    ));
    documents.extend(Daemon::ALL.iter().filter_map(|command| {
        detail(*command).map(|text| {
            let help = if *command == Daemon::Help {
                "worminald --help".to_owned()
            } else {
                format!("worminald {} --help", command.name())
            };
            (help, text)
        })
    }));
    documents.extend(crate::api_help::documents());
    documents
}

fn service_command(scope: &[u8], command: Option<Daemon>) -> io::Result<Option<i32>> {
    match command {
        Some(Daemon::Serve) => {
            process::prepare_service()?;
            crate::serve(scope)?;
        }
        Some(Daemon::UpgradeReceiver | Daemon::HandoffReceiver) => {
            process::prepare_service()?;
            crate::service::receive_upgrade(scope, command == Some(Daemon::HandoffReceiver))?;
        }
        Some(Daemon::Bridge | Daemon::BridgeExisting) => {
            bridge(scope, command == Some(Daemon::BridgeExisting))?;
        }
        Some(Daemon::Stop) => {
            let socket = connect(scope)?;
            Session::new(socket.try_clone()?, socket).send(&Command::new(0, CommandKind::Stop))?;
        }
        _ => return Ok(None),
    }
    Ok(Some(0))
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    let scope = std::env::var_os("WORMINAL_SHARED_SOCKET_SCOPE").unwrap_or_default();
    let scope = scope.as_os_str().as_bytes();
    let command = arguments
        .get(1)
        .and_then(|value| value.to_str())
        .and_then(Daemon::parse);
    if arguments.len() == 3 && Daemon::Help.matches(&arguments[2]) {
        if let Some(command) = command {
            print!("{}", cli_output::text(reference(command)));
            return Ok(0);
        }
    }
    match command {
        Some(Daemon::SshChild) => return crate::remote_connect::ssh_child(&arguments[2..]),
        Some(Daemon::ConnectMaster) => return crate::remote_connect::run(&arguments[2..]),
        Some(Daemon::InstallRemote) => return crate::remote_install::run(&arguments[2..], false),
        Some(Daemon::UpgradeRemote) => return crate::remote_install::run(&arguments[2..], true),
        Some(Daemon::Config) => return crate::config_cli::run(&arguments[2..]),
        Some(Daemon::ListMasters) => return crate::master_history::run(&arguments[2..]),
        _ => {}
    }
    if arguments.len() == 2 && command == Some(Daemon::Version) {
        println!(
            "{}",
            cli_output::text(format!("worminald {}", env!("CARGO_PKG_VERSION")))
        );
        return Ok(0);
    }
    if command == Some(Daemon::Api) {
        return crate::api_cli::run(scope, &arguments[2..]);
    }
    if arguments.len() == 2 && command == Some(Daemon::Help) {
        print!("{}", cli_output::text(reference(Daemon::Help)));
        return Ok(0);
    }
    if arguments.len() == 2 {
        if let Some(status) = service_command(scope, command)? {
            return Ok(status);
        }
    }
    if arguments.len() == 3 {
        let kind = match command {
            Some(Daemon::Handin) => Some(WIRE_HANDIN),
            Some(Daemon::Handoff) => Some(WIRE_HANDOFF),
            Some(Daemon::NewBrowser) => Some(WIRE_BROWSER_REPLACE),
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
                    "{}", cli_output::error("Handoff unavailable or rejected; no automatic daemon restart was attempted.")
                );
            }
            return result;
        }
    }
    eprintln!(
        "{}",
        cli_output::error(
            "usage: worminald api COMMAND (run 'worminald api help' for commands)\n\
               worminald --serve|--bridge|--stop\n\
               worminald --handin NAME | --handoff NAME\n       \
               worminald --new-browser-tab URL|FILE"
        )
    );
    Ok(2)
}
