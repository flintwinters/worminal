use super::*;
use std::{
    io::{Cursor, Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::net::UnixStream,
    thread,
};

struct Echo;
impl Handler for Echo {
    fn handle(&mut self, request: Request) -> io::Result<Reply> {
        Ok(Reply {
            success: true,
            json: format!("{{\"ok\":true,\"tab\":{}}}", request.tab()),
        })
    }
}

fn requests() -> Vec<Request> {
    vec![
        Request::List,
        Request::Devices,
        Request::Enroll {
            device: devices::Device {
                name: "bench".into(),
                endpoint: "felix@bench".into(),
            },
        },
        Request::Forget {
            name: "bench".into(),
        },
        Request::Read { tab: 12 },
        Request::Input {
            tab: 12,
            bytes: b"hello\r".to_vec(),
        },
    ]
}

fn exercise_stream<R, W>(reader: R, writer: W)
where
    R: Read,
    W: Write,
{
    let mut client = Client::new(stream::Stream::new(reader, writer));
    let mut direct = Client::new(Direct(Echo));
    for request in requests() {
        assert_eq!(
            client.request(request.clone()).unwrap(),
            direct.request(request).unwrap()
        );
    }
}

#[test]
fn identical_worp_operations_work_over_unix_and_tcp_streams() {
    let (client, mut server) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || {
        for _ in requests() {
            stream::serve_once(server.try_clone().unwrap(), &mut server, &mut Echo).unwrap();
        }
    });
    exercise_stream(client.try_clone().unwrap(), client);
    worker.join().unwrap();

    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let worker = thread::spawn(move || {
        let (mut server, _) = listener.accept().unwrap();
        for _ in requests() {
            stream::serve_once(server.try_clone().unwrap(), &mut server, &mut Echo).unwrap();
        }
    });
    exercise_stream(client.try_clone().unwrap(), client);
    worker.join().unwrap();
}

#[test]
fn v2_binding_roundtrips_and_validates_before_dispatch() {
    for request in requests() {
        assert_eq!(
            v2::decode_request(&v2::encode_request(&request).unwrap()).unwrap(),
            request
        );
    }
    let bad = Request::Input {
        tab: 12,
        bytes: vec![0; MAX_INPUT_BYTES + 1],
    };
    assert!(Client::new(Direct(Echo)).request(bad).is_err());
    assert!(Client::new(Direct(Echo))
        .request(Request::Read { tab: 0 })
        .is_err());
    let reply = Echo.handle(Request::List).unwrap();
    let packet = v2::encode_reply(12, &reply).unwrap();
    assert!(v2::decode_reply(13, packet).is_err());
}

#[test]
fn uncertain_stream_delivery_never_retries_input() {
    let request = Request::Input {
        tab: 1,
        bytes: b"run\r".to_vec(),
    };
    let mut written = Vec::new();
    let result = Client::new(stream::Stream::new(
        Cursor::new(Vec::<u8>::new()),
        &mut written,
    ))
    .request(request.clone());
    assert!(result.is_err());
    assert_eq!(
        written,
        v2::encode_request(&request).unwrap().encode().unwrap()
    );
}
