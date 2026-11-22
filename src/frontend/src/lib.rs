//! Rust-owned frontend state. The release module remains C until its replacement is complete.
pub mod app;
mod askpass;
mod boundary;
mod browser;
pub mod fonts;
mod graphics_ffi;
pub mod input;
pub mod layout;
mod link;
mod link_action;
pub mod paint;
mod startup;
pub mod tabs;
pub mod terminal;
pub mod viewport;
mod wsl_frame;

pub(crate) fn timing(event: &str) {
    if std::env::var_os("WORMINAL_TRACE_TIMING").is_some() {
        eprintln!("worminal-timing {event} {}", glib::monotonic_time() * 1000);
    }
}

/// Run a widget test on the one thread GTK accepts; libtest gives every test its own thread. Tests run on
/// a private display (`manage.py check-rust-frontend`) and fail rather than skip without one.
#[cfg(test)]
pub(crate) fn with_gtk(test: impl FnOnce() + Send + 'static) {
    use std::{
        panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
        sync::{mpsc, Mutex, OnceLock},
    };
    type Job = Box<dyn FnOnce() + Send>;
    static GTK: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    let jobs = GTK.get_or_init(|| {
        let (send, receive) = mpsc::channel::<Job>();
        std::thread::spawn(move || {
            gtk::init().expect("GTK tests need a display");
            receive.into_iter().for_each(|job| job());
        });
        Mutex::new(send)
    });
    let (done, result) = mpsc::channel();
    let job: Job = Box::new(move || {
        let _ = done.send(catch_unwind(AssertUnwindSafe(test)));
    });
    jobs.lock().unwrap().send(job).expect("GTK test thread");
    if let Err(panic) = result.recv().expect("GTK test thread") {
        resume_unwind(panic);
    }
}
