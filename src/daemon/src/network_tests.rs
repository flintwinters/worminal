//! Deterministic service/transport workloads, driven by manage.py network-perf and daemon checks.
use super::*;
use std::{io::Write, os::unix::net::UnixStream, thread, time::Duration};

struct Fixture {
    service: Service,
    _peer: UnixStream,
}
impl Fixture {
    fn new() -> Self {
        Self::with_features(9)
    }
    fn with_features(features: u32) -> Self {
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
        launch.columns = 120;
        launch.rows = 40;
        launch.arguments = vec!["/bin/cat".into()];
        service
            .tabs
            .push(Tab::shell(1, &launch, false, Some(1), service.reaper.clone()).unwrap());
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
        self.service.tabs[0].terminal.as_mut().unwrap().pty.stop();
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

#[derive(Default)]
struct Receiver {
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
