//! Terminal applications' desktop notifications (OSC 99), shown by the session's notification service.
use super::{App, WINDOWS};
use glib::{prelude::*, VariantTy};
use gtk::{gdk, gio, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};
use worminal_protocol::worp::events::{Notification, Occasion};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// Installed desktop entry and icon name; services use it to attribute and group notifications.
const APPLICATION: &str = "worminal";

/// Each tab's latest notification by service id: the next one replaces it, and a click finds its tab.
pub(super) type Shown = RefCell<HashMap<u32, u32>>;

thread_local! { static SUBSCRIBED: Cell<bool> = const { Cell::new(false) }; }

/// Applications control this text. Services may render body markup, so it is sent escaped.
fn plain(bytes: &[u8]) -> String {
    let text: String = String::from_utf8_lossy(bytes)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    text.trim().to_owned()
}

/// Clicking a notification selects its tab and raises that window.
fn subscribe(connection: &gio::DBusConnection) {
    if SUBSCRIBED.replace(true) {
        return;
    }
    connection.signal_subscribe(
        Some(SERVICE),
        Some(SERVICE),
        Some("ActionInvoked"),
        Some(PATH),
        None,
        gio::DBusSignalFlags::NONE,
        |_, _, _, _, _, parameters| {
            let Some((id, _)) = parameters.get::<(u32, String)>() else {
                return;
            };
            let target = WINDOWS.with(|windows| {
                windows.borrow().iter().find_map(|app| {
                    let shown = app.notices.borrow();
                    let tab = shown.iter().find(|(_, shown)| **shown == id)?.0;
                    Some((app.clone(), *tab))
                })
            });
            if let Some((app, tab)) = target.filter(|(app, _)| !app.closing.get()) {
                app.select_id(tab);
                crate::graphics_ffi::present_window(&app.window);
            }
        },
    );
}

impl App {
    /// The tab already blinks; this adds the reason where the user is looking, if the session can show it.
    pub(super) fn notify(self: &Rc<Self>, tab: u32, notification: &Notification) {
        let selected = self.active.get() == tab;
        let iconified = self
            .window
            .window()
            .is_some_and(|window| window.state().contains(gdk::WindowState::ICONIFIED));
        let wanted = match notification.occasion {
            Occasion::Always => true,
            Occasion::Unfocused => !(selected && self.window.is_active()),
            Occasion::Invisible => !(selected && self.window.is_visible() && !iconified),
        };
        let (mut summary, mut body) = (plain(&notification.title), plain(&notification.body));
        if summary.is_empty() {
            std::mem::swap(&mut summary, &mut body);
        }
        if !wanted || summary.is_empty() {
            return;
        }
        let replaces = self.notices.borrow().get(&tab).copied().unwrap_or(0);
        let hints = HashMap::from([
            ("urgency", (notification.urgency as u8).to_variant()),
            ("desktop-entry", APPLICATION.to_variant()),
        ]);
        let parameters = (
            "Worminal",
            replaces,
            APPLICATION,
            summary,
            glib::markup_escape_text(&body).as_str(),
            vec!["default", "Show"],
            hints,
            -1i32,
        )
            .to_variant();
        let app = Rc::downgrade(self);
        // A session without a bus or notification service keeps the blinking tab as its only signal.
        gio::bus_get(gio::BusType::Session, gio::Cancellable::NONE, move |bus| {
            let Ok(connection) = bus else { return };
            subscribe(&connection);
            connection.call(
                Some(SERVICE),
                PATH,
                SERVICE,
                "Notify",
                Some(&parameters),
                Some(VariantTy::new("(u)").expect("notification reply type")),
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
                move |reply| {
                    let id = reply.ok().and_then(|reply| reply.get::<(u32,)>());
                    if let (Some(app), Some((id,))) = (app.upgrade(), id) {
                        app.notices.borrow_mut().insert(tab, id);
                    }
                },
            );
        });
    }
}
