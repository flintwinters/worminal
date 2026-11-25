use super::{commands::*, events::*, launch::Launch, session::Session, v2_commands, v2_events, *};
use crate::*;
use std::io::{Cursor, Write};

fn variants() -> Vec<CommandKind> {
    let mut kinds = vec![
        CommandKind::Open {
            launch: Launch::restore(vec![b'/', 0xff]),
            mode: OpenMode::Subscribe,
        },
        CommandKind::Open {
            launch: Launch::restore(vec![b'/', 0xff]),
            mode: OpenMode::ReuseRestored,
        },
        CommandKind::Open {
            launch: Launch::restore(vec![b'/', 0xff]),
            mode: OpenMode::Detached,
        },
        CommandKind::Browser {
            url: b"https://example.test/".to_vec(),
            subscribe: true,
        },
        CommandKind::Browser {
            url: b"https://example.test/".to_vec(),
            subscribe: false,
        },
        CommandKind::Navigate(b"https://next.test/".to_vec()),
        CommandKind::ReplaceBrowser(b"https://next.test/".to_vec()),
        CommandKind::Input {
            bytes: b"hello\r".to_vec(),
            control: Control::Acquire,
        },
        CommandKind::Input {
            bytes: b"hello\r".to_vec(),
            control: Control::Preserve,
        },
        CommandKind::Focus {
            columns: 80,
            rows: 24,
        },
        CommandKind::Resize {
            columns: 90,
            rows: 25,
        },
        CommandKind::Pane {
            columns: 40,
            rows: 12,
        },
        CommandKind::FocusState(true),
        CommandKind::Close { receipt: false },
        CommandKind::Close { receipt: true },
        CommandKind::Scroll {
            up: true,
            amount: 5,
        },
        CommandKind::Control(ControlAction::Break),
        CommandKind::Control(ControlAction::PrintScreen),
        CommandKind::Control(ControlAction::TogglePrinter),
        CommandKind::Selection {
            generation: 42,
            operation: SelectionOperation::Start,
        },
        CommandKind::Selection {
            generation: 42,
            operation: SelectionOperation::Next,
        },
        CommandKind::Selection {
            generation: 42,
            operation: SelectionOperation::Cancel,
        },
        CommandKind::Move { position: 3 },
        CommandKind::Release,
        CommandKind::Attention(2),
        CommandKind::Subscribe,
        CommandKind::Capabilities(7),
        CommandKind::ServiceInfo,
        CommandKind::Master {
            operation: MasterOperation::Detach,
            host: b"user@build".to_vec(),
            launch: None,
        },
        // The launch trails the host: a terminal to open once the master is attached.
        CommandKind::Master {
            operation: MasterOperation::Attach,
            host: b"build".to_vec(),
            launch: Some(Box::new(Launch::restore(b"/srv".to_vec()))),
        },
        CommandKind::Stop,
        CommandKind::RecoverTabs(1 << 35),
        CommandKind::Transfer {
            outbound: true,
            token: b"x".to_vec(),
        },
        CommandKind::Call(Request::Read { tab: 7 }),
    ];
    kinds.extend(input_variants());
    kinds
}

fn input_variants() -> Vec<CommandKind> {
    vec![
        CommandKind::Key {
            keysym: 0x63,
            modifiers: input::MOD_CONTROL,
            character: Some('c'),
            numlock: true,
        },
        CommandKind::Key {
            keysym: 0xff52,
            modifiers: 0,
            character: None,
            numlock: false,
        },
        CommandKind::Mouse {
            button: input::MOUSE_NONE,
            column: 3,
            row: 4,
            modifiers: input::MOD_SHIFT,
            action: MouseAction::Motion,
        },
        CommandKind::Mouse {
            button: 0,
            column: 0,
            row: 0,
            modifiers: 0,
            action: MouseAction::Release,
        },
        CommandKind::Paste {
            bytes: b"a\r\nb".to_vec(),
            begin: true,
            end: false,
        },
        CommandKind::Paste {
            bytes: Vec::new(),
            begin: false,
            end: true,
        },
        CommandKind::Wheel {
            up: true,
            notches: 2,
            lines: 3,
            column: 5,
            row: 6,
            modifiers: input::MOD_ALT,
        },
    ]
}

#[test]
fn canonical_commands_roundtrip_through_installed_v2_binding() {
    for kind in variants() {
        let tab = if matches!(
            kind,
            CommandKind::Open {
                mode: OpenMode::Detached,
                ..
            } | CommandKind::Browser {
                subscribe: false,
                ..
            }
        ) {
            0
        } else {
            7
        };
        let command = Command::new(tab, kind);
        assert_eq!(
            v2_commands::decode(v2_commands::encode(&command).unwrap()).unwrap(),
            Some(command)
        );
    }
}

#[test]
fn unary_and_stream_clients_decode_to_identical_mutation_contracts() {
    let options = tabs::TerminalOptions {
        title: "same operation".into(),
        ..Default::default()
    };
    let request = Request::OpenTerminal {
        options: options.clone(),
    };
    let command = v2_commands::decode(v2::encode_request(&request).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        command,
        Command::new(
            0,
            CommandKind::Open {
                launch: Launch::from_options(&options),
                mode: OpenMode::Detached
            }
        )
    );
    let input = v2_commands::decode(
        v2::encode_request(&Request::Input {
            tab: 7,
            bytes: b"x".to_vec(),
        })
        .unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        input,
        Command::new(
            7,
            CommandKind::Input {
                bytes: b"x".to_vec(),
                control: Control::Preserve
            }
        )
    );
    let close = v2_commands::decode(v2::encode_request(&Request::Close { tab: 7 }).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(close, Command::new(7, CommandKind::Close { receipt: true }));
}

#[test]
fn v2_bytes_are_unchanged_and_malformed_commands_are_rejected() {
    let command = Command::new(
        7,
        CommandKind::Resize {
            columns: 80,
            rows: 24,
        },
    );
    let packet = v2_commands::encode(&command).unwrap();
    assert_eq!(packet.kind, WIRE_RESIZE);
    assert_eq!(
        packet.body,
        [80u32.to_be_bytes(), 24u32.to_be_bytes()].concat()
    );
    for (kind, body) in [
        (WIRE_RESIZE, vec![0; 7]),
        // Bounds hold for every decoded command, not only the input kinds.
        (
            WIRE_RESIZE,
            [0u32.to_be_bytes(), 24u32.to_be_bytes()].concat(),
        ),
        (
            WIRE_SCROLL,
            [2u32.to_be_bytes(), 1u32.to_be_bytes()].concat(),
        ),
        (
            WIRE_SCROLL,
            [1u32.to_be_bytes(), (i32::MAX as u32 + 1).to_be_bytes()].concat(),
        ),
        (WIRE_FOCUS_STATE, 2u32.to_be_bytes().to_vec()),
        (WIRE_CONTROL, 3u32.to_be_bytes().to_vec()),
        (
            WIRE_KEY,
            [0u32, 0, 0xd800, 0].map(u32::to_be_bytes).concat(),
        ),
        (WIRE_KEY, [0u32, 0, 0, 2].map(u32::to_be_bytes).concat()),
        (
            WIRE_MOUSE,
            [0u32, 0, 0, 0, 3].map(u32::to_be_bytes).concat(),
        ),
        (WIRE_PASTE, 4u32.to_be_bytes().to_vec()),
        (
            WIRE_PASTE,
            [&0u32.to_be_bytes()[..], &[b'x'; input::MAX_PASTE_BYTES + 1]].concat(),
        ),
        (
            WIRE_WHEEL,
            [0u32, 1, i32::MAX as u32 + 1, 0, 0, 0]
                .map(u32::to_be_bytes)
                .concat(),
        ),
        (
            WIRE_WHEEL,
            [0u32, input::MAX_WHEEL_NOTCHES + 1, 1, 0, 0, 0]
                .map(u32::to_be_bytes)
                .concat(),
        ),
    ] {
        assert!(v2_commands::decode(Packet { kind, tab: 7, body }).is_err());
    }
    assert!(v2_commands::encode(&Command::new(
        7,
        CommandKind::Input {
            bytes: vec![0; MAX_INPUT_BYTES + 1],
            control: Control::Acquire
        }
    ))
    .is_err());
    assert!(v2_commands::encode(&Command::new(
        7,
        CommandKind::Resize {
            columns: 0,
            rows: 24
        }
    ))
    .is_err());
}

#[test]
fn session_receives_ordered_zero_body_events_and_sends_without_retry() {
    assert_eq!(
        v2_events::decode(Packet {
            kind: WIRE_COMMAND_DONE,
            tab: 0,
            body: vec![0; 4]
        })
        .unwrap(),
        Some(Event {
            tab: 0,
            kind: EventKind::CommandDone
        })
    );
    let events = [
        Event {
            tab: 7,
            kind: EventKind::Created,
        },
        Event {
            tab: 7,
            kind: EventKind::FrameFinish,
        },
        Event {
            tab: 7,
            kind: EventKind::Bell,
        },
    ];
    let bytes = events
        .iter()
        .flat_map(|event| v2_events::encode(event).unwrap().encode().unwrap())
        .collect::<Vec<_>>();
    let mut session = Session::new(Cursor::new(bytes), Vec::new());
    session
        .send(&Command::new(0, CommandKind::Subscribe))
        .unwrap();
    for event in events {
        assert_eq!(session.receive().unwrap(), event);
    }
    assert!(session.receive().is_err());
    let (_, output) = session.into_parts();
    assert_eq!(
        output,
        Packet {
            kind: WIRE_ATTACH,
            tab: 0,
            body: vec![]
        }
        .encode()
        .unwrap()
    );
    struct Broken(usize);
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut session = Session::new(Cursor::new(Vec::<u8>::new()), Broken(0));
    assert!(session
        .send(&Command::new(7, CommandKind::Close { receipt: false }))
        .is_err());
    assert_eq!(session.into_parts().1 .0, 1);
}

#[test]
fn persistent_contract_is_identical_over_unix_and_tcp() {
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        os::unix::net::UnixStream,
        thread,
    };
    fn serve(mut reader: impl Read, mut writer: impl Write) {
        let command = v2_commands::decode(stream::read_packet(&mut reader).unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(command, Command::new(0, CommandKind::Subscribe));
        let event = Event {
            tab: 0,
            kind: EventKind::Catalog(Catalog {
                closed: 0,
                fallback: 0,
                tabs: vec![],
            }),
        };
        writer
            .write_all(&v2_events::encode(&event).unwrap().encode().unwrap())
            .unwrap();
    }
    fn client(reader: impl Read, writer: impl Write) {
        let mut session = Session::new(reader, writer);
        session
            .send(&Command::new(0, CommandKind::Subscribe))
            .unwrap();
        assert_eq!(
            session.receive().unwrap(),
            Event {
                tab: 0,
                kind: EventKind::Catalog(Catalog {
                    closed: 0,
                    fallback: 0,
                    tabs: vec![]
                })
            }
        );
    }
    let (a, b) = UnixStream::pair().unwrap();
    let worker = thread::spawn(move || serve(b.try_clone().unwrap(), b));
    client(a.try_clone().unwrap(), a);
    worker.join().unwrap();
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let a = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let worker = thread::spawn(move || {
        let (b, _) = listener.accept().unwrap();
        serve(b.try_clone().unwrap(), b)
    });
    client(a.try_clone().unwrap(), a);
    worker.join().unwrap();
}
