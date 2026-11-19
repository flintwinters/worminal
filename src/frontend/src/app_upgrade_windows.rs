//! Candidate windows reconcile to the frozen donor snapshot while reusing unchanged warm links.
use super::*;

struct Prepared {
    id: u32,
    configuration: Vec<u8>,
    app: Rc<App>,
    view: gui_handoff::Presentation,
}
pub(super) struct Candidate {
    windows: Vec<Prepared>,
    catalogs: Vec<UpdateBarrier>,
}
impl Candidate {
    pub(super) fn new() -> Self {
        Self {
            windows: Vec::new(),
            catalogs: Vec::new(),
        }
    }
    pub(super) fn reconcile(&mut self, states: Vec<Window>, candidate: bool) -> io::Result<()> {
        let mut prepared = Vec::new();
        for state in states {
            let configuration = gui_handoff::configuration(std::slice::from_ref(&state))?;
            let app = if let Some(index) = self
                .windows
                .iter()
                .position(|old| old.id == state.id && old.configuration == configuration)
            {
                self.windows.remove(index).app
            } else {
                open(&state, candidate)?
            };
            app.close_after_update
                .set(state.flags & gui_handoff::CLOSE_REQUESTED != 0);
            apply(&app, state.presentation());
            prepared.push(Prepared {
                id: state.id,
                configuration,
                app,
                view: state.presentation(),
            });
        }
        // Create all replacements before retiring provisional windows, including when every
        // original window closed during warm-up. GTK must never see an intermediate empty owner.
        for old in self.windows.drain(..) {
            unsafe {
                old.app.window.destroy();
            }
        }
        self.windows = prepared;
        self.catalogs.clear();
        for window in &self.windows {
            self.catalogs.extend(window.app.update_barriers());
        }
        Ok(())
    }
    pub(super) fn ready(&self) -> bool {
        // An unreachable peer cannot settle; waiting for it would postpone every update until it
        // returns (a dead master timed out each GUI replacement). The candidate keeps retrying it.
        barriers_complete(&self.catalogs)
            && self.windows.iter().all(|window| {
                window
                    .app
                    .links()
                    .iter()
                    .all(|link| link.settled() || link.down())
            })
    }
    pub(super) fn select(&self) {
        for window in &self.windows {
            // A shell may exit independently while committing. Use the daemon's existing
            // fallback rather than rejecting the GUI transaction or resurrecting that shell.
            if window
                .app
                .pages
                .borrow()
                .iter()
                .any(|page| page.id == window.view.selected)
            {
                window.app.select_id(window.view.selected);
            }
        }
    }
    pub(super) fn finish(&mut self, bytes: &[u8]) -> io::Result<()> {
        if !bytes.is_empty() {
            let views = gui_handoff::decode_presentations(bytes)?;
            if views.len() != self.windows.len() {
                return Err(io::Error::other("Invalid final window count"));
            }
            for view in views {
                let window = self
                    .windows
                    .iter_mut()
                    .find(|window| window.id == view.id)
                    .ok_or_else(|| io::Error::other("Invalid final window identity"))?;
                apply(&window.app, view);
                window.view = view;
            }
        }
        self.select();
        Ok(())
    }
    pub(super) fn focus(&self) -> Option<u32> {
        self.windows
            .iter()
            .find(|window| window.view.flags & 4 != 0)
            .map(|window| window.app.state_id)
    }
    pub(super) fn closing(&self, id: u32) -> bool {
        self.windows.iter().any(|window| {
            window.app.state_id == id
                && (window.view.flags & gui_handoff::CLOSE_REQUESTED != 0
                    || window.app.close_after_update.get())
        })
    }
    pub(super) fn close_requested(&self) {
        for window in &self.windows {
            if window.view.flags & gui_handoff::CLOSE_REQUESTED != 0
                || window.app.close_after_update.replace(false)
            {
                window.app.window.close();
            }
        }
    }
}
fn open(state: &Window, candidate: bool) -> io::Result<Rc<App>> {
    let Some(mut config) = Config::parse(&state.request, candidate)? else {
        return Err(io::Error::other("Invalid update launch"));
    };
    config.attach = true;
    config.command.clear();
    config.browser_url = None;
    config.zoom = state.zoom;
    let app = App::open_mode(
        config,
        true,
        state.flags & gui_handoff::PRIMARY_DETACHED != 0,
    )?;
    for link in &state.links {
        let Some(mut config) = Config::parse(&link.request, candidate)? else {
            return Err(io::Error::other("Invalid remote update launch"));
        };
        config.attach = true;
        config.command.clear();
        config.browser_url = None;
        app.restore_remote(link.index, config)?;
    }
    Ok(app)
}
fn apply(app: &Rc<App>, view: gui_handoff::Presentation) {
    if app.fonts.borrow().zoom() != view.zoom {
        app.set_zoom(view.zoom);
    }
    app.geometry_pending.set(false);
    app.window
        .set_default_size(view.geometry[2], view.geometry[3]);
    app.window.resize(view.geometry[2], view.geometry[3]);
    if crate::graphics_ffi::x11(&app.window) {
        app.window.move_(view.geometry[0], view.geometry[1]);
    }
    if view.flags & 1 != 0 {
        app.window.maximize();
    } else {
        app.window.unmaximize();
    }
    if view.flags & 2 != 0 {
        app.window.fullscreen();
    } else {
        app.window.unfullscreen();
    }
}
