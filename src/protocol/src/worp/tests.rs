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
        Request::ReadMany {
            tabs: vec![12, 13],
            tail: 200,
        },
        Request::Keys {
            tab: 12,
            names: vec!["Ctrl-Left".into(), "F5".into()],
            guard: "identity".into(),
        },
        Request::Paste {
            tab: 12,
            text: "one\ntwo".into(),
            guard: String::new(),
        },
        Request::GuardedInput {
            tab: 12,
            bytes: b"literal".to_vec(),
            guard: "identity".into(),
        },
        Request::Wait {
            tab: 12,
            after: "revision".into(),
            timeout_ms: 1000,
        },
        Request::OpenTerminal {
            options: tabs::TerminalOptions {
                directory: "/target".into(),
                title: "quoted tab".into(),
                command: vec!["/bin/echo".into(), "literal ; $HOME".into()],
                ..Default::default()
            },
        },
        Request::OpenBrowser {
            url: "https://example.test/".into(),
        },
        Request::Close { tab: 12 },
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
fn uncertain_stream_delivery_never_retries_mutations() {
    for request in [
        Request::Input {
            tab: 1,
            bytes: b"run\r".to_vec(),
        },
        Request::OpenTerminal {
            options: Default::default(),
        },
        Request::OpenBrowser {
            url: "about:blank".into(),
        },
        Request::Close { tab: 1 },
    ] {
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
}

#[test]
fn lifecycle_validation_rejects_ambiguous_or_unbounded_targets() {
    assert!(Request::Close { tab: 0 }.validate().is_err());
    for url in ["", "relative.html", "https://host/\0hidden"] {
        assert!(Request::OpenBrowser { url: url.into() }.validate().is_err());
    }
    for options in [
        tabs::TerminalOptions {
            directory: "relative".into(),
            ..Default::default()
        },
        tabs::TerminalOptions {
            columns: 0,
            ..Default::default()
        },
        tabs::TerminalOptions {
            command: vec!["/bin/sh\0hidden".into()],
            ..Default::default()
        },
    ] {
        assert!(Request::OpenTerminal { options }.validate().is_err());
    }
}

#[test]
fn extended_requests_reject_invalid_bounds_and_trailing_data() {
    for request in [
        Request::ReadMany {
            tabs: vec![],
            tail: 1,
        },
        Request::ReadMany {
            tabs: vec![1; 17],
            tail: 1,
        },
        Request::ReadMany {
            tabs: vec![0],
            tail: 1,
        },
        Request::ReadMany {
            tabs: vec![1],
            tail: 1001,
        },
        Request::Wait {
            tab: 1,
            after: "r".into(),
            timeout_ms: 60001,
        },
        Request::Keys {
            tab: 1,
            names: vec![],
            guard: String::new(),
        },
        Request::Paste {
            tab: 1,
            text: "\x1b[201~".into(),
            guard: String::new(),
        },
    ] {
        assert!(request.validate().is_err());
    }
    for request in requests().into_iter().filter(|r| {
        matches!(
            r,
            Request::ReadMany { .. }
                | Request::Keys { .. }
                | Request::Paste { .. }
                | Request::GuardedInput { .. }
                | Request::Wait { .. }
        )
    }) {
        let mut packet = v2::encode_request(&request).unwrap();
        packet.body.push(0);
        assert!(v2::decode_request(&packet).is_err());
    }
}
