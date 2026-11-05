//! Repeatable hot-path workloads; invoked only through manage.py daemon-perf.
use crate::{
    queue::Queue,
    state::{Screen, Tab},
};
use std::{hint::black_box, time::Instant};
use worminal_engine::Engine;

fn measure(name: &str, iterations: usize, mut operation: impl FnMut()) {
    let started = Instant::now();
    for _ in 0..iterations {
        operation();
    }
    println!("DAEMON_PERF {name} {} {iterations}", started.elapsed().as_nanos());
}

#[test]
#[ignore = "release hot-path measurement through manage.py daemon-perf"]
fn hot_paths() {
    let mut queue = Queue::new(2 * 1024 * 1024);
    queue.append(&vec![0; 1024 * 1024]).unwrap();
    let bytes = [1; 1024];
    measure("congested_queue", 4000, || {
        assert_eq!(
            queue.flush(&mut std::io::sink(), bytes.len()).unwrap(),
            bytes.len()
        );
        queue.append(&bytes).unwrap();
        black_box(queue.pending());
    });
    let mut tabs: Vec<_> = (0..64)
        .map(|id| Tab::browser(id, b"https://example.test".to_vec()))
        .collect();
    for tab in &mut tabs {
        tab.metadata(1).unwrap();
    }
    measure("cached_catalog_64_tabs", 200, || {
        let mut catalog = worminal_protocol::Buffer::with_limit(usize::MAX);
        for tab in &mut tabs {
            catalog.put_bytes(&tab.metadata(1).unwrap()).unwrap();
        }
        black_box(catalog);
    });
    let mut engine = Engine::new(100, 40).unwrap();
    let mut screen = Screen::default();
    let mut output = Queue::new(4 * 1024 * 1024);
    measure("full_frame_100x40", 500, || {
        screen
            .replicate(&mut engine, &mut output, 1, true, false, false, 1)
            .unwrap();
        output.flush(&mut std::io::sink(), usize::MAX).unwrap();
        black_box(&screen.bytes);
    });
}
