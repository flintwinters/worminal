//! Remembering each window's layout (`docs/layout-restore.md`): the split tree, divider positions, which tabs sit
//! in which pane, and the window's size and position, so a frontend that was closed by accident or crashed
//! reopens as it was.
//!
//! A saved layout names tabs by position in the daemon's order, because tab ids are new after a daemon
//! restart. Saving never reads the screen mid-change: layout changes only schedule one debounced write. A
//! window restores once, when its first catalog lists its tabs; a link that was lost and comes back restores the
//! last good layout again, because a restarted daemon hands out new tab ids and the panes would otherwise
//! collapse. While the link is down nothing is saved, so a lost daemon never overwrites the layout it held.
use super::{App, LINK_SHIFT, WINDOWS};
use crate::layout::Layout;
use gtk::{gdk, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};
use worminal_workspace::layout_state::{self, Node, Saved, Window};

/// How long layout changes settle before they are written.
const SETTLE: Duration = Duration::from_millis(500);

/// One window's layout bookkeeping.
#[derive(Default)]
pub(super) struct Persist {
    /// The saved window to rebuild once this window's tabs are known.
    pending: RefCell<Option<Window>>,
    /// The window takes part in saving: restoring is decided and the file is trustworthy.
    enabled: Cell<bool>,
    scheduled: Rc<Cell<bool>>,
    /// Closed on purpose with its layout forgotten.
    discarded: Cell<bool>,
    /// Size and position while not maximized.
    normal: Cell<[i32; 4]>,
    /// The last layout seen while the link was up, to rebuild after a lost daemon comes back.
    last_good: RefCell<Option<Window>>,
}

enum Store {
    Unread,
    /// Saved windows no window has taken yet.
    Ready(VecDeque<Window>),
    /// The file could not be read as a layout: it stays as it is and nothing is saved over it.
    Damaged,
}

thread_local! {
    static STORE: RefCell<Store> = const { RefCell::new(Store::Unread) };
}

fn layout_path() -> std::io::Result<PathBuf> {
    layout_state::path(&worminal_config::state_home(), &super::scope())
}

impl App {
    /// Take this window's saved layout, if any, and start following its geometry. Called once the panes exist.
    pub(super) fn begin_restore(self: &Rc<Self>, staged: bool) {
        if !worminal_config::get().restore_layout {
            return;
        }
        if staged {
            // A window replacement's new windows take their split from the handoff (`App::arrange`), not from the
            // file: it carries remote tabs too, and works with this setting off. They save once they own the window.
            self.follow_geometry();
            self.panes.persist.enabled.set(true);
            return;
        }
        let Some((window, extra)) = STORE.with(|store| {
            let mut store = store.borrow_mut();
            if matches!(*store, Store::Unread) {
                *store = match layout_path().and_then(|path| layout_state::load(&path)) {
                    Ok(saved) => {
                        Store::Ready(saved.map(|saved| saved.windows).unwrap_or_default().into())
                    }
                    Err(error) => {
                        eprintln!("worminal: {error}");
                        self.message(Some(&error.to_string()));
                        Store::Damaged
                    }
                };
            }
            match &mut *store {
                Store::Ready(windows) => Some((windows.pop_front(), windows.len())),
                _ => None,
            }
        }) else {
            return;
        };
        self.follow_geometry();
        if let Some(saved) = window {
            self.place(&saved);
            *self.panes.persist.pending.borrow_mut() = Some(saved);
        } else {
            self.panes.persist.enabled.set(true);
        }
        // The rest of a crashed session's windows open beside this one, each taking the next saved layout.
        if extra > 0 {
            let config = super::Config {
                attach: true,
                ..self.config.clone()
            };
            glib::idle_add_local_once(move || {
                for _ in 0..extra {
                    match App::open(config.clone()) {
                        Ok(app) => app.join_connected_masters(),
                        Err(error) => eprintln!("worminal: cannot reopen a window: {error}"),
                    }
                }
            });
        }
    }

    fn follow_geometry(self: &Rc<Self>) {
        self.window.connect_configure_event(
            glib::clone!(@weak self as app => @default-return false, move |window, _| {
                if !window.is_maximized() {
                    let (x, y) = window.position();
                    let (width, height) = window.size();
                    app.panes.persist.normal.set([x, y, width, height]);
                }
                app.layout_changed();
                false
            }),
        );
        self.window.connect_is_maximized_notify(
            glib::clone!(@weak self as app => move |_| app.layout_changed()),
        );
    }

    /// Give the window its saved size and position (the window manager has the last word) and maximize it
    /// again. A position on a monitor that is gone is not used.
    fn place(&self, saved: &Window) {
        let [x, y, width, height] = saved.geometry;
        self.geometry_pending.set(false);
        self.window.set_default_size(width, height);
        self.panes.persist.normal.set(saved.geometry);
        let visible = gdk::Display::default().is_some_and(|display| {
            (0..display.n_monitors())
                .filter_map(|index| display.monitor(index))
                .any(|monitor| {
                    let area = monitor.geometry();
                    (area.x()..area.x() + area.width() - 40).contains(&x)
                        && (area.y()..area.y() + area.height() - 40).contains(&y)
                })
        });
        if visible {
            self.window.move_(x, y);
        }
        if saved.maximized {
            self.window.maximize();
        }
    }

    /// This window as it is now, or `None` before it has tabs or while its daemon is lost.
    fn layout_snapshot(&self) -> Option<Window> {
        // A window still being handed over is not the owner yet: the donor keeps the file current until commit.
        if self.primary.down()
            || self.replacing.get()
            || self.panes.persist.pending.borrow().is_some()
        {
            return None;
        }
        // A tab's position is its place among this machine's tabs, in daemon order.
        let local: Vec<u32> = self
            .pages
            .borrow()
            .iter()
            .filter(|page| page.id >> LINK_SHIFT == 0)
            .map(|page| page.id)
            .collect();
        if local.is_empty() {
            return None;
        }
        let position = |id: u32| {
            local
                .iter()
                .position(|tab| *tab == id)
                .map(|index| index as u32)
        };
        let tree = self.panes.layout.borrow().to_node(&|id| {
            let Some(pane) = self.pane(id) else {
                return Node::pane(Vec::new());
            };
            Node::Pane {
                tabs: self
                    .tabs_of(&pane)
                    .into_iter()
                    .filter_map(position)
                    .collect(),
                selected: self.active_of(&pane).and_then(position),
            }
        });
        let leaves = self.panes.layout.borrow().leaves();
        let focused = leaves
            .iter()
            .position(|id| *id == self.panes.focused.get())
            .unwrap_or(0) as u32;
        let maximized = self.window.is_maximized();
        let mut geometry = self.panes.persist.normal.get();
        if geometry[2] < 1 || geometry[3] < 1 {
            let (width, height) = self.window.size();
            let (x, y) = self.window.position();
            geometry = [x, y, width, height];
        }
        let window = Window {
            id: self.state_id,
            geometry,
            maximized,
            tree,
            focused,
        };
        *self.panes.persist.last_good.borrow_mut() = Some(window.clone());
        Some(window)
    }

    /// Something about the layout changed: write it once the changes settle.
    pub(super) fn layout_changed(&self) {
        let persist = &self.panes.persist;
        if !persist.enabled.get() || persist.scheduled.replace(true) {
            return;
        }
        let scheduled = persist.scheduled.clone();
        glib::timeout_add_local_once(SETTLE, move || {
            scheduled.set(false);
            save_all();
        });
    }

    /// Write the layout now, so a window closing within the settle time loses nothing.
    pub(super) fn flush_layout(&self) {
        if self.panes.persist.enabled.get() {
            save_all();
        }
    }

    /// Rebuild the saved layout over the tabs that exist, once this window's first catalog has arrived (or a
    /// lost daemon is back).
    pub(super) fn restore_layout(self: &Rc<Self>, reconnected: bool, requested: u32) {
        let pending = self.panes.persist.pending.borrow_mut().take();
        let Some(saved) = pending.or_else(|| {
            reconnected
                .then(|| self.panes.persist.last_good.borrow().clone())
                .flatten()
        }) else {
            return;
        };
        let local: Vec<u32> = self
            .pages
            .borrow()
            .iter()
            .filter(|page| page.id >> LINK_SHIFT == 0)
            .map(|page| page.id)
            .collect();
        if local.is_empty() {
            *self.panes.persist.pending.borrow_mut() = Some(saved);
            return;
        }
        let tree = saved.tree.clone().fit(local.len() as u32);
        // Start from one pane holding every tab, then deal them out.
        let home = self.focused_pane();
        let all: Vec<u32> = self.pages.borrow().iter().map(|page| page.id).collect();
        for id in all {
            self.move_page(id, &home);
        }
        self.panes
            .list
            .borrow_mut()
            .retain(|pane| pane.id == home.id);
        self.focus_pane(&home);
        let mut deal = Vec::new();
        let mut reused = false;
        let layout = Layout::from_node(&tree, &mut |node| {
            let pane = if reused {
                self.add_pane()
            } else {
                home.clone()
            };
            reused = true;
            if let Node::Pane { tabs, selected } = node {
                deal.push((pane.clone(), tabs.clone(), *selected));
            }
            pane.id
        });
        *self.panes.layout.borrow_mut() = layout;
        for (pane, tabs, _) in &deal {
            for tab in tabs {
                self.move_page(local[*tab as usize], pane);
            }
        }
        self.reorder_panes();
        self.rebuild();
        let was = self.syncing.replace(true);
        for (pane, _, selected) in &deal {
            let widget = selected.and_then(|tab| self.page_widget(local[tab as usize]));
            if let Some(widget) = widget {
                pane.notebook
                    .set_current_page(pane.notebook.page_num(&widget));
            }
        }
        self.syncing.set(was);
        let leaves = self.panes.layout.borrow().leaves();
        let focus = leaves
            .get(saved.focused as usize)
            .or(leaves.first())
            .and_then(|id| self.pane(*id));
        // A window launched to show a tab (`worminal -e`, a new tab beside running ones) keeps showing it: the
        // restore places the tabs around it but must not select the tab that was focused last time.
        let tab = (requested != 0 && self.page_widget(requested).is_some())
            .then_some(requested)
            .or_else(|| focus.and_then(|pane| self.active_of(&pane)));
        if let Some(tab) = tab {
            self.select_id(tab);
        }
        if !self.replacing.get() {
            self.present(true);
        }
        self.panes.persist.enabled.set(true);
    }

    /// Close this window and forget its layout, so the next start does not bring it back.
    pub(super) fn close_without_layout(self: &Rc<Self>) {
        self.panes.persist.discarded.set(true);
        save_all();
        self.window.close();
    }
}

/// Write every window's layout, in the order the windows opened. A window not restored yet holds the
/// write back, so it never replaces layouts the process has not rebuilt.
fn save_all() {
    let apps = WINDOWS.with(|windows| windows.borrow().clone());
    let live: Vec<_> = apps
        .iter()
        .filter(|app| !app.closing.get() && !app.panes.persist.discarded.get())
        .collect();
    if live
        .iter()
        .any(|app| app.panes.persist.pending.borrow().is_some())
    {
        return;
    }
    let windows: Vec<Window> = live
        .iter()
        .filter(|app| app.panes.persist.enabled.get())
        .filter_map(|app| app.layout_snapshot())
        .collect();
    let result = layout_path().and_then(|path| {
        if windows.is_empty() {
            // Only forget when a window was dropped on purpose; a process with nothing to save leaves the file.
            if apps.iter().any(|app| app.panes.persist.discarded.get()) {
                return layout_state::forget(&path);
            }
            return Ok(());
        }
        layout_state::store(&path, &Saved { windows })
    });
    if let Err(error) = result {
        eprintln!("worminal: layout not saved: {error}");
    }
}
