//! Deterministic service/transport workloads, driven by manage.py network-perf and daemon checks.
use super::*;
use std::{fs::File, io::Write, os::unix::net::UnixStream, path::Path, thread, time::Duration};

/// manage.py network-perf sets this directory to estimate SSH-compressed wire bytes.
const STREAMS: &str = "WORMINAL_NETWORK_STREAMS";

/// Records each write as one length-prefixed chunk, approximating one SSH packet.
struct Chunks(File);
impl Write for Chunks {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write_all(&(bytes.len() as u32).to_be_bytes())?;
        self.0.write_all(bytes)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The delivered protocol stream of workload `name`, retained only under network-perf.
fn stream(name: &str) -> Box<dyn Write> {
    match std::env::var_os(STREAMS) {
        Some(directory) => Box::new(Chunks(
            File::create(Path::new(&directory).join(name)).unwrap(),
        )),
        None => Box::new(io::sink()),
    }
}

struct Fixture {
    service: Service,
    _peer: UnixStream,
}
impl Fixture {
    fn new() -> Self {
        Self::with_features(9)
    }
    fn with_features(features: u32) -> Self {
        Self::sized(features, 120, 40, 1)
    }
    fn sized(features: u32, columns: u32, rows: u32, tabs: u32) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let scope = format!("network-{}-{serial}", std::process::id());
        let (mut service, _) = create(
            scope.as_bytes(),
            socket::listen(process::uid(), scope.as_bytes()).unwrap(),
        )
        .unwrap();
        let (stream, peer) = UnixStream::pair().unwrap();
        service.clients.insert(
            1,
            Client {
                id: 1,
                socket: stream,
                pid: 0,
                ready: false,
                features: 0,
                decoder: Decoder::default(),
                output: Queue::new(CLIENT_QUEUE),
                needs_resync: false,
                selection: None,
            },
        );
        // Bit 3 is ROW_SPAN. Literal permits the identical workload to run on pre-feature revisions;
        // their real capability handler ignores it, just as it would for a newer network client.
        service
            .message(
                1,
                Packet {
                    kind: WIRE_CAPABILITIES,
                    tab: 0,
                    body: features.to_be_bytes().to_vec(),
                },
            )
            .unwrap();
        service.clients.get_mut(&1).unwrap().ready = true;
        service
            .clients
            .get_mut(&1)
            .unwrap()
            .output
            .flush(&mut io::sink(), usize::MAX)
            .unwrap();
        let mut launch = Launch::restore(Vec::new());
        launch.columns = columns;
        launch.rows = rows;
        launch.arguments = vec!["/bin/cat".into()];
        for id in 1..=tabs {
            service
                .tabs
                .push(Tab::shell(id, &launch, false, Some(1), service.reaper.clone()).unwrap());
        }
        Self {
            service,
            _peer: peer,
        }
    }
    fn output(&mut self) -> &mut Queue {
        &mut self.service.clients.get_mut(&1).unwrap().output
    }
    fn feed(&mut self, bytes: &[u8], tick: usize) {
        self.service.tabs[0]
            .feed(bytes, tick as i64 * 16_000_000 + 1)
            .unwrap();
    }
    fn send(&mut self, tick: usize) {
        self.service
            .send_frame(0, false, tick as i64 * 16_000_000 + 1)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for tab in &mut self.service.tabs {
            tab.terminal.as_mut().unwrap().pty.stop();
        }
        for _ in 0..200 {
            if self.service.reaper.reap() == 0 {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("network fixture child did not exit");
    }
}

#[test]
fn congestion_retains_partial_bytes_and_resumes_with_a_delta() {
    let mut fixture = Fixture::new();
    fixture.send(1);
    fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
    fixture.output().append(&vec![7; 128 * 1024]).unwrap();
    fixture.output().flush(&mut io::sink(), 17).unwrap();
    fixture.output().append(&[7; 17]).unwrap();
    let queued = fixture.output().pending().to_vec();
    fixture.feed(b"newest", 2);
    fixture.send(2);
    assert!(fixture.service.clients[&1].needs_resync);
    assert_eq!(fixture.output().pending(), queued);
    fixture.output().flush(&mut io::sink(), 64 * 1024).unwrap();
    fixture.service.resync(48_000_001).unwrap();
    assert!(
        fixture.service.clients[&1].needs_resync,
        "resume must honor the low watermark"
    );
    fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
    fixture.service.resync(64_000_001).unwrap();
    assert!(!fixture.service.clients[&1].needs_resync);
    let mut bytes = fixture.output().pending();
    let mut decoder = Decoder::default();
    let mut rows = 0;
    while !bytes.is_empty() {
        let (used, packet) = decoder.push(bytes).unwrap();
        bytes = &bytes[used..];
        if let Some(packet) = packet {
            rows += usize::from(packet.kind == WIRE_ROW || packet.kind == 32);
        }
    }
    assert_eq!(rows, 1, "resuming must not resend all 40 rows");
}

#[test]
fn deferred_full_requests_survive_the_congestion_gate() {
    let mut fixture = Fixture::new();
    fixture.send(1);
    fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
    fixture.output().append(&vec![0; 128 * 1024]).unwrap();
    fixture.service.send_frame(0, true, 32_000_001).unwrap();
    assert!(fixture.service.tabs[0].screen.bytes.is_empty());
    fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
    fixture.service.resync(48_000_001).unwrap();
    assert_eq!(fixture.output().len(), 77_656);
}

/// Attach idle tabs at 187x82 (247040-byte full frames) that all remain controlled, as hidden tabs
/// do after activation, then drain a 2 Mbps link. Returns delivered protocol bytes and settlement.
/// Before 2fc0825, resync() sent FULL frames for every controlled tab. Once their sum exceeded the
/// then 1 MiB backlog, the last tabs were gated and re-armed resync, which resent every full frame:
/// idle hidden tabs streamed at link rate until disconnect.
const IDLE_TABS: usize = 6;
fn idle_controlled_tabs(ticks: usize) -> (usize, bool) {
    let mut fixture = Fixture::sized(9, 187, 82, IDLE_TABS as u32);
    for index in 0..IDLE_TABS {
        fixture.service.send_frame(index, true, 1).unwrap();
    }
    let mut bytes = 0;
    let mut wire = stream("idle_hidden_tabs");
    for tick in 1..=ticks {
        bytes += fixture.output().flush(&mut wire, 4096).unwrap();
        fixture
            .service
            .resync(tick as i64 * 16_000_000 + 1)
            .unwrap();
    }
    let settled = fixture.output().is_empty() && !fixture.service.clients[&1].needs_resync;
    (bytes, settled)
}

#[test]
fn idle_controlled_tabs_settle_after_one_full_frame_each() {
    let (bytes, settled) = idle_controlled_tabs(3000);
    let full = 82 * (187 * 16 + 4);
    assert!(
        settled,
        "idle tabs must stop sending once their baselines are delivered"
    );
    assert!(
        bytes < 2 * IDLE_TABS * full,
        "idle tabs resent full frames: {bytes} bytes"
    );
}

/// Two 120x40 tabs in one window; tab 2 animates a status line while tab 1 is displayed, then the
/// window reveals tab 2. Drives only wire packets so network-perf can replay it on old revisions.
/// Returns (bytes while hidden, bytes to reveal). Callers pass FOCUSED_FRAMES as the literal bit 16
/// (bit 4), so network-perf replays the identical workload on revisions predating it.
fn hidden_busy_tab(features: u32, ticks: usize) -> (usize, usize) {
    let mut fixture = Fixture::sized(features, 120, 40, 2);
    let focus = |fixture: &mut Fixture, tab: u32| {
        let body = [120u32, 40].iter().flat_map(|v| v.to_be_bytes()).collect();
        fixture
            .service
            .message(
                1,
                Packet {
                    kind: WIRE_FOCUS,
                    tab,
                    body,
                },
            )
            .unwrap();
        fixture.output().flush(&mut io::sink(), usize::MAX).unwrap()
    };
    focus(&mut fixture, 2);
    focus(&mut fixture, 1);
    let mut hidden = 0;
    let mut wire = stream("hidden_busy_tab");
    // WIRE_FOCUS reads the service clock; the engine rejects time that moves backwards.
    for _ in 0..ticks {
        let now = fixture.service.now();
        fixture.service.tabs[1]
            .feed(format!("\x1b[40H working {now:020}").as_bytes(), now)
            .unwrap();
        fixture.service.send_frame(1, false, now).unwrap();
        hidden += fixture.output().flush(&mut wire, usize::MAX).unwrap();
    }
    let revealed = focus(&mut fixture, 2);
    (hidden, revealed)
}

/// The frontend's tab blink for BEL (`WIRE_BELL`) exists for background tabs, which are exactly the
/// tabs whose frames are deferred; the bell must not depend on the frame gate.
#[test]
fn hidden_tab_bell_still_reaches_its_controller() {
    let mut fixture = Fixture::sized(9 | 16, 120, 40, 2);
    for tab in [2, 1] {
        let body = [120u32, 40].iter().flat_map(|v| v.to_be_bytes()).collect();
        let packet = Packet {
            kind: WIRE_FOCUS,
            tab,
            body,
        };
        fixture.service.message(1, packet).unwrap();
    }
    assert!(!fixture.service.tabs[1].shown, "tab 2 must be hidden");
    let now = fixture.service.now();
    let notices = fixture.service.tabs[1].feed(b"done\x07", now).unwrap();
    assert!(
        notices.iter().any(|n| matches!(
            n,
            Notice::Event(1, worminal_protocol::worp::events::EventKind::Bell)
        )),
        "a hidden controlled tab must still notify its controller of BEL"
    );
}

#[test]
fn hidden_tabs_defer_frames_and_reveal_with_a_delta() {
    let full = 40 * (120 * 16 + 4);
    let (hidden, revealed) = hidden_busy_tab(9 | 16, 100);
    assert_eq!(hidden, 0, "a hidden tab must not stream frames");
    assert!(
        revealed > 0 && revealed < full / 4,
        "reveal must send a delta: {revealed} bytes"
    );
    let (hidden, _) = hidden_busy_tab(9, 100);
    assert!(
        hidden > 0,
        "clients without the feature still receive every controlled tab"
    );
}

#[test]
fn move_command_reorders_tabs_and_publishes_the_new_order() {
    let mut fixture = Fixture::sized(9, 80, 24, 3);
    let ids =
        |fixture: &Fixture| -> Vec<u32> { fixture.service.tabs.iter().map(|t| t.id).collect() };
    let send = |fixture: &mut Fixture, tab: u32, position: u32| {
        fixture
            .service
            .message(
                1,
                Packet {
                    kind: WIRE_MOVE,
                    tab,
                    body: position.to_be_bytes().to_vec(),
                },
            )
            .unwrap();
    };
    send(&mut fixture, 1, 2);
    assert_eq!(ids(&fixture), [2, 3, 1]);
    send(&mut fixture, 1, 0);
    assert_eq!(ids(&fixture), [1, 2, 3]);
    send(&mut fixture, 2, 99);
    assert_eq!(ids(&fixture), [1, 3, 2], "positions clamp to the end");
    send(&mut fixture, 77, 0);
    assert_eq!(ids(&fixture), [1, 3, 2], "unknown tabs are ignored");
    assert!(
        !fixture.service.clients[&1].output.is_empty(),
        "the new order reaches ready views as a catalog"
    );
}

#[derive(Default)]
struct Receiver {
    stream: Option<Box<dyn Write>>,
    decoder: Decoder,
    cells: Vec<u8>,
    bytes: usize,
    tick: usize,
    frames: usize,
    ages: Vec<usize>,
}
impl Write for Receiver {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let count = bytes.len();
        self.bytes += count;
        if let Some(stream) = &mut self.stream {
            stream.write_all(bytes)?;
        }
        self.cells.resize(120 * 40 * 16, 0);
        while !bytes.is_empty() {
            let (used, packet) = self.decoder.push(bytes)?;
            bytes = &bytes[used..];
            let Some(packet) = packet else {
                continue;
            };
            let number = |offset| {
                u32::from_be_bytes(packet.body[offset..offset + 4].try_into().unwrap()) as usize
            };
            match packet.kind {
                WIRE_ROW => {
                    let start = number(0) * 120 * 16;
                    self.cells[start..start + packet.body.len() - 4]
                        .copy_from_slice(&packet.body[4..]);
                }
                WIRE_COPY_ROWS => {
                    self.cells.copy_within(
                        number(0) * 1920..(number(0) + number(8)) * 1920,
                        number(4) * 1920,
                    );
                }
                32 => {
                    let start = (number(0) * 120 + number(4)) * 16;
                    self.cells[start..start + packet.body.len() - 8]
                        .copy_from_slice(&packet.body[8..]);
                }
                WIRE_FRAME_FINISH => {
                    self.frames += 1;
                    let text: String = self.cells[..128]
                        .chunks_exact(16)
                        .map(|cell| {
                            char::from_u32(u32::from_be_bytes(cell[..4].try_into().unwrap()))
                                .unwrap()
                        })
                        .collect();
                    if let Ok(source) = text.parse::<usize>() {
                        self.ages.push(self.tick - source);
                    }
                }
                _ => {}
            }
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn metric(name: &str, value: usize) {
    println!("NETWORK_PERF {name} {value}");
}

#[test]
#[ignore = "deterministic transport model through manage.py network-perf"]
fn transport_model() {
    let mut fixture = Fixture::new();
    fixture.send(0);
    let mut receiver = Receiver::default();
    fixture.output().flush(&mut receiver, usize::MAX).unwrap();
    receiver.stream = Some(stream("sparse"));
    let mut sparse_bytes = 0;
    for tick in 1..=100 {
        fixture.feed(format!("\x1b[H{:08}", tick).as_bytes(), tick);
        fixture.send(tick);
        receiver.tick = tick;
        sparse_bytes += fixture.output().flush(&mut receiver, usize::MAX).unwrap();
    }
    metric("sparse_bytes_per_frame", sparse_bytes / 100);
    drop(fixture);
    let mut fixture = Fixture::new();
    fixture.send(0);
    let mut receiver = Receiver::default();
    fixture.output().flush(&mut receiver, usize::MAX).unwrap();
    receiver.bytes = 0;
    receiver.frames = 0;
    receiver.stream = Some(stream("congested"));
    let mut peak = 0;
    // 250 ticks of 16 ms; drain 4096 raw protocol bytes/tick (2.048 Mbps).
    // Dense unique rows avoid row-move savings. Time is virtual, not measured SSH/pixel latency.
    for tick in 1..=250 {
        let mut text = format!("\x1b[H{:08}", tick);
        for row in 0..40 {
            text.push_str(&format!("\x1b[{};9H", row + 1));
            for col in 8..120 {
                text.push((33 + (tick * 17 + row * 13 + col * 7) % 90) as u8 as char);
            }
        }
        fixture.feed(text.as_bytes(), tick);
        fixture.send(tick);
        peak = peak.max(fixture.output().len());
        receiver.tick = tick;
        fixture.output().flush(&mut receiver, 4096).unwrap();
        fixture
            .service
            .resync(tick as i64 * 16_000_000 + 1)
            .unwrap();
    }
    let mut tick = 250;
    while !fixture.output().is_empty() || fixture.service.clients[&1].needs_resync {
        tick += 1;
        assert!(tick < 2000);
        receiver.tick = tick;
        fixture.output().flush(&mut receiver, 4096).unwrap();
        fixture
            .service
            .resync(tick as i64 * 16_000_000 + 1)
            .unwrap();
    }
    assert_eq!(receiver.cells, fixture.service.tabs[0].screen.bytes);
    let engine = &mut fixture.service.tabs[0].terminal.as_mut().unwrap().engine;
    let mut expected = Vec::new();
    let mut cells = Vec::new();
    for row in 0..40 {
        engine.row(row, &mut cells).unwrap();
        for cell in &cells {
            for field in [cell.rune, cell.attributes, cell.foreground, cell.background] {
                expected.extend_from_slice(&field.to_be_bytes());
            }
        }
    }
    assert_eq!(
        receiver.cells, expected,
        "coalesced transport must catch up to actual engine cells"
    );
    receiver.ages.sort_unstable();
    metric("congested_peak_queue_bytes", peak);
    metric(
        "congested_p95_frame_age_ms",
        receiver.ages[(receiver.ages.len() * 95).div_ceil(100) - 1] * 16,
    );
    metric("congested_tail_ms", (tick - 250) * 16);
    metric("congested_protocol_bytes", receiver.bytes);
    metric("congested_frames", receiver.frames);
    // 3000 ticks of 16 ms at 4096 bytes/tick cap this at 12288000 bytes (link saturation).
    metric("idle_hidden_tabs_bytes", idle_controlled_tabs(3000).0);
    let (hidden, revealed) = hidden_busy_tab(9 | 16, 1000);
    metric("hidden_busy_tab_bytes", hidden);
    metric("hidden_busy_reveal_bytes", revealed);
}

#[test]
fn row_spans_require_negotiation_and_dense_rows_keep_legacy_encoding() {
    for features in [0u32, 1, 8, 9] {
        let mut fixture = Fixture::with_features(features);
        fixture.send(0);
        fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
        fixture.feed(b"!", 1);
        fixture.send(1);
        let spans = features & 8 != 0;
        assert_eq!(fixture.output().len(), if spans { 96 } else { 1996 });
        fixture.output().flush(&mut io::sink(), usize::MAX).unwrap();
        fixture.feed(format!("\x1b[H{}", "X".repeat(120)).as_bytes(), 2);
        fixture.send(2);
        assert_eq!(
            fixture.output().len(),
            1996,
            "dense updates must avoid span header overhead"
        );
    }
}

#[test]
fn automation_reads_and_types_without_replacing_the_controller() {
    let mut fixture = Fixture::sized(0, 20, 4, 1);
    let (stream, _agent_peer) = UnixStream::pair().unwrap();
    fixture.service.clients.insert(
        2,
        Client {
            id: 2,
            socket: stream,
            pid: 0,
            ready: false,
            features: 0,
            decoder: Decoder::default(),
            output: Queue::new(CLIENT_QUEUE),
            needs_resync: false,
            selection: None,
        },
    );
    fixture.service.tabs[0]
        .feed(b"\x1b[2J\x1b[Hhello\r\nworld", 1)
        .unwrap();
    let controller = fixture.service.tabs[0].controller;
    for operation in [WORP_LIST, WORP_READ] {
        let tab = u32::from(operation != WORP_LIST);
        fixture
            .service
            .message(
                2,
                Packet {
                    kind: WIRE_WORP,
                    tab,
                    body: operation.to_be_bytes().to_vec(),
                },
            )
            .unwrap();
    }
    let mut body = WORP_INPUT.to_be_bytes().to_vec();
    body.extend_from_slice(b"message");
    fixture
        .service
        .message(
            2,
            Packet {
                kind: WIRE_WORP,
                tab: 1,
                body,
            },
        )
        .unwrap();
    assert_eq!(fixture.service.tabs[0].controller, controller);
    assert_eq!(
        fixture.service.tabs[0]
            .terminal
            .as_ref()
            .unwrap()
            .input
            .pending(),
        b"message"
    );
    let mut decoder = Decoder::default();
    let mut bytes = fixture.service.clients.get(&2).unwrap().output.pending();
    let mut replies = Vec::new();
    while !bytes.is_empty() {
        let (used, packet) = decoder.push(bytes).unwrap();
        bytes = &bytes[used..];
        if let Some(packet) = packet.filter(|packet| packet.kind == WIRE_WORP) {
            assert_eq!(&packet.body[..4], &1u32.to_be_bytes());
            replies.push(String::from_utf8(packet.body[4..].to_vec()).unwrap());
        }
    }
    assert!(replies[0].contains("\"id\":1"));
    assert!(replies[1].contains("\"lines\":[\"hello\",\"world\""));
    assert!(replies[2].contains("\"queued_bytes\":7"));
}

#[test]
fn automation_queue_rejection_keeps_the_terminal_alive() {
    let mut fixture = Fixture::sized(0, 20, 4, 1);
    let terminal = fixture.service.tabs[0].terminal.as_mut().unwrap();
    terminal
        .input
        .append(&vec![b'x'; terminal.input.remaining() - 1])
        .unwrap();
    let queued = terminal.input.len();
    let mut body = WORP_INPUT.to_be_bytes().to_vec();
    body.extend_from_slice(b"\r");
    fixture
        .service
        .message(
            1,
            Packet {
                kind: WIRE_WORP,
                tab: 1,
                body,
            },
        )
        .unwrap();
    let terminal = fixture.service.tabs[0].terminal.as_ref().unwrap();
    assert_eq!(terminal.input.len(), queued);
    assert!(terminal.pty.fd().is_some());
    assert!(fixture
        .output()
        .pending()
        .windows(15)
        .any(|bytes| bytes == b"queue is full.\""));
}

#[test]
fn osc99_only_reaches_capable_controllers() {
    use worminal_protocol::worp::events::{Event, EventKind};
    for features in [0, WIRE_FEATURE_NOTIFICATIONS] {
        let mut fixture = Fixture::with_features(features);
        let now = fixture.service.now();
        let notices = fixture.service.tabs[0]
            .feed(b"\x1b]99;;Build;done\x07", now)
            .unwrap();
        let mut delivered = false;
        for notice in notices {
            if let Notice::Event(client, kind @ EventKind::Notification(_)) = notice {
                assert_eq!(client, 1);
                assert_eq!(kind, EventKind::Notification(b";Build;done".to_vec()));
                fixture.service.emit(client, Event { tab: 1, kind });
                delivered = true;
            }
        }
        assert!(delivered);
        let mut bytes = Vec::new();
        fixture.output().flush(&mut bytes, usize::MAX).unwrap();
        assert_eq!(bytes.is_empty(), features == 0);
        if features != 0 {
            let (_, packet) = Decoder::default().push(&bytes).unwrap();
            assert_eq!(
                worp::v2_events::decode(packet.unwrap()).unwrap(),
                Some(Event {
                    tab: 1,
                    kind: EventKind::Notification(b";Build;done".to_vec())
                })
            );
        }
    }
}
