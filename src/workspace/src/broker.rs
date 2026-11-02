//! Shared launch forwarding envelope. The owner receives caller state without mutating its own process environment.
use crate::rights;
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
    peer.set_read_timeout(Some(Duration::from_secs(5)))?;
    peer.set_write_timeout(Some(Duration::from_secs(5)))?;
    rights::send_descriptor(&peer, request.output.as_fd(), 0)?;
    peer.write_all(&request.encode()?)?;
    let mut result = [0];
    peer.read_exact(&mut result)?;
    if result[0] == 0 {
        Ok(Forward::Complete)
    } else {
        Err(io::Error::other("GTK owner rejected launch"))
    }
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
