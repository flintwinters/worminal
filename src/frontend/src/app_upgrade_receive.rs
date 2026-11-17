//! Candidate side of the ordered GUI replacement transaction.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Warming,
    Prepared,
    Importing,
    Live,
    Mapping,
    Receiving,
    Refreshing,
    Committing,
}

#[expect(
    clippy::too_many_lines,
    reason = "candidate handshake and GTK event loop form one ordered transaction"
)]
pub(crate) fn receive(token: &OsStr, candidate: bool) -> io::Result<()> {
    let token = token
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid update token"))?;
    let mut peer = UnixStream::connect_addr(&address(token)?)?;
    peer.set_read_timeout(Some(LIMIT))?;
    let (listener, lease, states) = gui_handoff::receive(&mut peer)?;
    crate::browser::configure_profile(candidate)?;
    gtk::init().map_err(io::Error::other)?;
    let _state = super::super::window_state::start();
    let mut prepared = candidate_windows::Candidate::new();
    prepared.reconcile(states, candidate)?;
    let refresh_address = address(token)?;
    let mut refresh: Option<Receiver<io::Result<(UnixListener, File, Vec<Window>)>>> = None;
    let connection = Rc::new(RefCell::new(channel(peer)?));
    let phase = Rc::new(Cell::new(Stage::Warming));
    let result = Rc::new(RefCell::new(None::<io::Result<Vec<u8>>>));
    let lease = Rc::new(RefCell::new(Some(lease)));
    let finished = Rc::new(Cell::new(false));
    let broker_slot = Rc::new(RefCell::new(None));
    let monitor_slot = Rc::new(RefCell::new(None));
    let listener = Rc::new(RefCell::new(Some(listener)));
    let mut deadline = Instant::now() + PREPARE_LIMIT;
    let mut atomic = false;
    let tick = glib::timeout_add_local(Duration::from_millis(25), {
        let connection = connection.clone();
        let phase = phase.clone();
        let result = result.clone();
        let lease = lease.clone();
        let finished = finished.clone();
        let broker_slot = broker_slot.clone();
        let monitor_slot = monitor_slot.clone();
        move || {
            let previous = phase.get();
            let action = (|| -> io::Result<bool> {
                connection.borrow_mut().flush()?;
                if phase.get() == Stage::Committing && connection.borrow().output.is_empty() {
                    finished.set(true);
                    prepared.close_requested();
                    return Ok(true);
                }
                let packets = connection.borrow_mut().read()?;
                for packet in packets {
                    match packet.kind {
                        REFRESH if phase.get() == Stage::Prepared => {
                            atomic = true;
                            let address = refresh_address.clone();
                            let (sender, receiver) = mpsc::sync_channel(1);
                            refresh = Some(receiver);
                            std::thread::spawn(move || {
                                let result = (|| {
                                    let mut peer = UnixStream::connect_addr(&address)?;
                                    peer.set_read_timeout(Some(LIMIT))?;
                                    gui_handoff::receive(&mut peer)
                                })();
                                let _ = sender.send(result);
                            });
                            deadline = Instant::now() + LIMIT;
                            phase.set(Stage::Receiving);
                        }
                        ACTIVATE if phase.get() == Stage::Prepared => {
                            if !packet.body.is_empty() {
                                browser::adopt(lease.borrow_mut().take().unwrap())?;
                                let result = result.clone();
                                browser::cookies(Some(&packet.body), move |value| {
                                    *result.borrow_mut() = Some(value)
                                })?;
                            } else {
                                *result.borrow_mut() = Some(Ok(Vec::new()));
                            }
                            phase.set(Stage::Importing);
                        }
                        FINISH if phase.get() == Stage::Live => {
                            prepared.finish(&packet.body)?;
                            let focus = prepared.focus();
                            for app in windows() {
                                app.handoff_frozen.set(false);
                                app.replacing.set(false);
                                if prepared.closing(app.state_id) {
                                    continue;
                                }
                                app.defer_navigation.set(false);
                                let pages = app.pages.borrow().clone();
                                for page in pages {
                                    if let Some(view) = page.view.borrow().as_ref() {
                                        view.navigate(&page.url);
                                    }
                                }
                                app.window.set_sensitive(true);
                                if Some(app.state_id) == focus {
                                    app.activate(app.active.get());
                                    app.window.present();
                                }
                                if !crate::graphics_ffi::x11(&app.window) {
                                    app.message(Some("Updated; window placement is controlled by the compositor."));
                                }
                            }
                            let broker =
                                broker::serve(listener.borrow_mut().take().unwrap(), candidate)?;
                            *monitor_slot.borrow_mut() = Some(watch(broker.clone(), candidate)?);
                            *broker_slot.borrow_mut() = Some(broker);
                            if atomic {
                                send(&mut connection.borrow_mut(), COMMITTED, Vec::new())?;
                                phase.set(Stage::Committing);
                            } else {
                                finished.set(true);
                                prepared.close_requested();
                                return Ok(true);
                            }
                        }
                        ABORT => return Err(io::Error::other("replacement cancelled")),
                        _ => return Err(io::Error::other("Invalid replacement handshake")),
                    }
                }
                if phase.get() == Stage::Receiving {
                    if let Some(receiver) = &refresh {
                        match receiver.try_recv() {
                            Ok(value) => {
                                let (new_listener, new_lease, states) = value?;
                                *listener.borrow_mut() = Some(new_listener);
                                *lease.borrow_mut() = Some(new_lease);
                                prepared.reconcile(states, candidate)?;
                                refresh = None;
                                phase.set(Stage::Refreshing);
                            }
                            Err(mpsc::TryRecvError::Empty) => {}
                            Err(_) => {
                                return Err(io::Error::other(
                                    "Authoritative snapshot transfer failed",
                                ))
                            }
                        }
                    }
                }
                if matches!(phase.get(), Stage::Warming | Stage::Refreshing) && prepared.ready() {
                    prepared.select();
                    if windows()
                        .iter()
                        .any(|app| app.pages.borrow().iter().any(|page| page.browser))
                    {
                        browser::preflight()?;
                    }
                    let capability = if phase.get() == Stage::Warming {
                        ATOMIC_STATE
                    } else {
                        FINAL_STATE
                    };
                    send(&mut connection.borrow_mut(), READY, capability.to_vec())?;
                    phase.set(Stage::Prepared);
                }
                if phase.get() == Stage::Importing {
                    let value = result.borrow_mut().take();
                    if let Some(value) = value {
                        value?;
                        for app in windows() {
                            app.window.set_sensitive(false);
                            app.handoff_frozen.set(true);
                            app.replacing.set(false);
                            crate::graphics_ffi::show_window(&app.window);
                            if app.active.get() != 0 {
                                app.activate(app.active.get());
                            }
                        }
                        phase.set(Stage::Mapping);
                    }
                }
                if phase.get() == Stage::Mapping
                    && windows().iter().all(|app| {
                        app.active.get() == 0
                            || app.replacement_frame.get()
                            || app
                                .pages
                                .borrow()
                                .iter()
                                .any(|page| page.id == app.active.get() && page.browser)
                    })
                {
                    if windows().iter().any(|app| {
                        app.pages
                            .borrow()
                            .iter()
                            .any(|page| page.id == app.active.get() && page.browser)
                            && !windows().iter().any(|owner| {
                                owner.config.master == app.config.master
                                    && owner.browser(app.active.get()).is_some()
                            })
                    }) {
                        return Err(io::Error::other("Candidate browser failed to initialize"));
                    }
                    send(&mut connection.borrow_mut(), LIVE, Vec::new())?;
                    phase.set(Stage::Live);
                }
                if Instant::now() > deadline {
                    return Err(io::Error::other("replacement handshake timed out"));
                }
                Ok(false)
            })();
            if previous != phase.get() {
                trace(&format!(
                    "GUI update candidate: {previous:?} -> {:?}",
                    phase.get()
                ));
            }
            match action {
                Ok(false) => glib::ControlFlow::Continue,
                Ok(true) => glib::ControlFlow::Break,
                Err(error) => {
                    worminal_workspace::diagnostics::log(
                        libc::LOG_ERR,
                        &format!("GUI update candidate failed: {error}"),
                    );
                    let _ = send(&mut connection.borrow_mut(), ABORT, Vec::new());
                    for app in windows() {
                        // Hidden staged windows have never mapped: close() would not destroy them.
                        unsafe {
                            app.window.destroy();
                        }
                    }
                    glib::MainContext::default().wakeup();
                    glib::ControlFlow::Break
                }
            }
        }
    });
    gtk::main();
    if !finished.get() {
        let _ = tick;
        return Err(io::Error::other("Window replacement did not commit"));
    }
    Ok(())
}
