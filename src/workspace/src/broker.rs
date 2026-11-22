//! Shared launch forwarding envelope. The owner receives caller state without mutating its own process environment.
use crate::{cli_schema::Launcher, rights};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::AsFd,
        unix::{
            ffi::OsStrExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::PathBuf,
    time::Duration,
};
use worminal_protocol::{Buffer, Decoder, Packet, Payload, WIRE_HELLO};

/// Commands only a running GUI owner can perform; they never open a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerAction<'a> {
    DetachMaster(&'a std::ffi::OsStr),
    CloseMaster(&'a std::ffi::OsStr),
    /// Bookmark the caller's tab: its shell's directory is the request's directory.
    Bookmark,
}

/// Owner-only commands take no launch options or shell command; master commands take one exact host.
pub fn owner_action(arguments: &[OsString]) -> io::Result<Option<OwnerAction<'_>>> {
    let command = match arguments
        .get(1)
        .and_then(|arg| arg.to_str())
        .and_then(Launcher::parse)
    {
        Some(command @ (Launcher::DetachMaster | Launcher::CloseMaster | Launcher::Bookmark)) => {
            command
        }
        _ => return Ok(None),
    };
    let usage = |operand: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("usage: worminal {}{operand}", command.name()),
        )
    };
    if command == Launcher::Bookmark {
        return if arguments.len() == 2 {
            Ok(Some(OwnerAction::Bookmark))
        } else {
            Err(usage(""))
        };
    }
    if arguments.len() != 3 || arguments[2].is_empty() || arguments[2].as_bytes()[0] == b'-' {
        return Err(usage(" user@host"));
    }
    let host = arguments[2].as_os_str();
    Ok(Some(if command == Launcher::DetachMaster {
        OwnerAction::DetachMaster(host)
    } else {
        OwnerAction::CloseMaster(host)
    }))
}

/// Display/workspace identity shared by lightweight launchers and direct frontend entry.
pub fn display_key(candidate: bool) -> Vec<u8> {
    let mut values = [
        "GDK_BACKEND",
        "WAYLAND_DISPLAY",
        "DISPLAY",
        "WORMINAL_SHARED_SOCKET_SCOPE",
    ]
    .map(|name| std::env::var_os(name).unwrap_or_default());
    // With only DISPLAY available, GTK's automatic backend is X11. Keep the existing
    // automatic-owner key so a shell exporting x11 can reach an already running owner.
    // On Wayland, explicit x11 can select a different display and must remain distinct.
    if values[0] == "x11" && values[1].is_empty() && !values[2].is_empty() {
        values[0].clear();
    }
    let mut key = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if candidate {
            key.extend_from_slice(&(value.as_bytes().len() as u64).to_be_bytes());
        } else if index != 0 {
            key.push(b':');
        }
        key.extend_from_slice(value.as_bytes());
    }
    key
}

pub struct Request {
    pub args: Vec<OsString>,
    pub directory: PathBuf,
    pub environment: Vec<OsString>,
    pub output: File,
}
impl Request {
    pub fn capture(args: Vec<OsString>) -> io::Result<Self> {
        Ok(Self {
            args,
            directory: std::env::current_dir()?,
            environment: std::env::vars_os()
                .map(|(key, value)| {
                    let mut entry = key;
                    entry.push("=");
                    entry.push(value);
                    entry
                })
                .collect(),
            output: File::from(io::stdout().as_fd().try_clone_to_owned()?),
        })
    }
    pub fn encode(&self) -> io::Result<Vec<u8>> {
        let mut body = Buffer::default();
        body.put_string(self.directory.as_os_str().as_bytes())?;
        body.put_u32(self.args.len().try_into().map_err(io::Error::other)?)?;
        for arg in &self.args {
            body.put_string(arg.as_bytes())?;
        }
        body.put_u32(
            self.environment
                .len()
                .try_into()
                .map_err(io::Error::other)?,
        )?;
        for entry in &self.environment {
            body.put_string(entry.as_bytes())?;
        }
        Packet {
            kind: WIRE_HELLO,
            tab: 0,
            body: body.into_bytes(),
        }
        .encode()
    }
    pub fn decode(packet: Packet, output: File) -> io::Result<Self> {
        use std::os::unix::ffi::OsStringExt;
        if packet.kind != WIRE_HELLO || packet.tab != 0 {
            return Err(io::Error::other("invalid launch envelope"));
        }
        let mut body = Payload::new(&packet.body);
        let directory = PathBuf::from(OsString::from_vec(body.string()?.to_vec()));
        fn strings(body: &mut Payload<'_>) -> io::Result<Vec<OsString>> {
            let count = body.u32()?;
            if count > 65536 {
                return Err(io::Error::other("too many launch strings"));
            }
            (0..count)
                .map(|_| Ok(OsString::from_vec(body.string()?.to_vec())))
                .collect()
        }
        let args = strings(&mut body)?;
        let environment = strings(&mut body)?;
        body.finish()?;
        if args.is_empty() || !directory.is_absolute() {
            return Err(io::Error::other("invalid launch request"));
        }
        Ok(Self {
            args,
            directory,
            environment,
            output,
        })
    }
}
/// The owner's verdict: one status byte, then on failure its bounded UTF-8 error text until EOF.
/// Owners that predate the text close after the byte, so callers fall back to a generic error.
const REPLY_TEXT_LIMIT: usize = 4096;
pub fn reply(result: &io::Result<()>) -> Vec<u8> {
    let mut bytes = vec![u8::from(result.is_err())];
    if let Err(error) = result {
        let text = error.to_string();
        let mut end = text.len().min(REPLY_TEXT_LIMIT);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        bytes.extend_from_slice(&text.as_bytes()[..end]);
    }
    bytes
}
fn verdict(peer: &mut impl Read) -> io::Result<Forward> {
    let mut result = [0];
    peer.read_exact(&mut result)?;
    if result[0] == 0 {
        return Ok(Forward::Complete);
    }
    let mut text = Vec::new();
    peer.take(REPLY_TEXT_LIMIT as u64).read_to_end(&mut text)?;
    let text = String::from_utf8_lossy(&text);
    Err(io::Error::other(if text.is_empty() {
        "GTK owner rejected launch; see its stderr".into()
    } else {
        text.into_owned()
    }))
}
pub enum Forward {
    Owner(UnixListener),
    Complete,
}
pub fn forward(address: &SocketAddr, request: &Request) -> io::Result<Forward> {
    match UnixListener::bind_addr(address) {
        Ok(listener) => return Ok(Forward::Owner(listener)),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
        Err(error) => return Err(error),
    }
    let mut peer = UnixStream::connect_addr(address)?;
    // A launcher can connect while GUI ownership is committing. Its accepted request
    // stays queued on the continuously bound listener, so outlive the bounded cutover.
    let timeout = Duration::from_secs(crate::gui_handoff::TIMEOUT_SECONDS + 5);
    peer.set_read_timeout(Some(timeout))?;
    peer.set_write_timeout(Some(timeout))?;
    rights::send_descriptor(&peer, request.output.as_fd(), 0)?;
    peer.write_all(&request.encode()?)?;
    verdict(&mut peer)
}
/// Incremental state used by GTK readiness sources; no GUI-thread blocking reads.
#[derive(Default)]
pub struct Incoming {
    output: Option<File>,
    decoder: Decoder,
}
impl Incoming {
    pub fn read(&mut self, peer: &mut UnixStream) -> io::Result<Option<Request>> {
        if self.output.is_none() {
            let descriptors = rights::receive_descriptors(peer.as_fd(), 0, 1)?;
            if descriptors.len() != 1 {
                return Err(io::Error::other("invalid launch descriptor"));
            }
            self.output = Some(File::from(descriptors.into_iter().next().unwrap()));
        }
        let mut bytes = [0; 16384];
        for _ in 0..4 {
            let count = peer.read(&mut bytes)?;
            if count == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let mut input = &bytes[..count];
            while !input.is_empty() {
                let (count, packet) = self.decoder.push(input)?;
                input = &input[count..];
                if let Some(packet) = packet {
                    if !input.is_empty() {
                        return Err(io::Error::other("trailing launch data"));
                    }
                    return Request::decode(packet, self.output.take().unwrap()).map(Some);
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_launches_carry_the_owners_error_text() {
        let usage =
            owner_action(&["worminal".into(), "--bookmark".into(), "x".into()]).unwrap_err();
        assert_eq!(usage.to_string(), "usage: worminal --bookmark");
        assert_eq!(
            owner_action(&["worminal".into(), "--bookmark".into()]).unwrap(),
            Some(OwnerAction::Bookmark)
        );
        let reply = reply(&Err(io::Error::other("Master felix is not connected")));
        let Err(error) = verdict(&mut reply.as_slice()) else {
            panic!("failure reply accepted");
        };
        assert_eq!(error.to_string(), "Master felix is not connected");
        assert!(matches!(
            verdict(&mut [0u8].as_slice()),
            Ok(Forward::Complete)
        ));
        // Owners predating error text send the bare status byte.
        let Err(error) = verdict(&mut [1u8].as_slice()) else {
            panic!("legacy failure accepted");
        };
        assert!(error.to_string().contains("see its stderr"));
    }
}
