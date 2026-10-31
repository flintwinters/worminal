use crate::{
    process,
    queue::Queue,
    reactor::{self, Interest},
};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    process::Child,
    thread,
    time::{Duration, Instant},
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

fn bridge(scope: &[u8]) -> io::Result<()> {
    let (mut socket, mut child) = ensure(scope)?;
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

fn reply(socket: &mut std::os::unix::net::UnixStream) -> io::Result<Packet> {
    let mut decoder = Decoder::default();
    let mut bytes = [0u8; 4096];
    loop {
        let count = socket.read(&mut bytes)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "workspace closed its reply",
            ));
        }
        let mut bytes = &bytes[..count];
        while !bytes.is_empty() {
            let (used, packet) = decoder.push(bytes)?;
            if let Some(packet) = packet {
                return Ok(packet);
            }
            bytes = &bytes[used..];
        }
    }
}

fn request(scope: &[u8], kind: u32, body: Vec<u8>) -> io::Result<i32> {
    let mut socket =
        connect(scope).map_err(|_| io::Error::other("No preview workspace service is running."))?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    socket.write_all(&Packet { kind, tab: 0, body }.encode()?)?;
    let packet = reply(&mut socket)?;
    if packet.kind == WIRE_COMMAND_DONE {
        return Ok(0);
    }
    let error = (packet.kind == WIRE_ERROR)
        .then(|| Payload::new(&packet.body).string().map(Vec::from))
        .transpose()?
        .unwrap_or_else(|| b"Preview service rejected the command.".to_vec());
    eprintln!("{}", String::from_utf8_lossy(&error));
    Ok(1)
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    let scope = std::env::var_os("WORMINAL_SHARED_SOCKET_SCOPE").unwrap_or_default();
    let scope = scope.as_os_str().as_bytes();
    if arguments.len() == 2 {
        match arguments[1].as_os_str().as_bytes() {
            b"--serve" => {
                process::prepare_service()?;
                crate::serve(scope)?;
                return Ok(0);
            }
            b"--bridge" => {
                bridge(scope)?;
                return Ok(0);
            }
            b"--stop" => {
                let mut socket = connect(scope)?;
                socket.write_all(
                    &Packet {
                        kind: WIRE_STOP,
                        tab: 0,
                        body: Vec::new(),
                    }
                    .encode()?,
                )?;
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
            let mut body = Buffer::default();
            body.put_string(&value)?;
            return request(scope, kind, body.into_bytes());
        }
    }
    eprintln!("usage: worminald --serve|--bridge|--stop\n       worminald --handin NAME | --handoff NAME\n       \
               worminald --new-browser-tab URL|FILE");
    Ok(2)
}
