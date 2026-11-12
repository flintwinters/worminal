//! Desktop notification lifetimes and click routing, independent of GTK window ownership.
use super::Message;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const MAX_IN_FLIGHT: usize = 32;
const MAX_TRACKED: usize = 128;
type Activate = Box<dyn FnOnce(Option<String>)>;
struct Tracked {
    id: u32,
    token: Option<String>,
    activate: Activate,
}
struct Connection {
    bus: gio::DBusConnection,
    signals: Vec<gio::SignalSubscriptionId>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        for signal in self.signals.drain(..) {
            self.bus.signal_unsubscribe(signal);
        }
    }
}
#[derive(Default)]
pub(in crate::app) struct Desktop {
    connection: RefCell<Option<Connection>>,
    tracked: RefCell<VecDeque<Tracked>>,
    in_flight: Cell<usize>,
    generation: Cell<u64>,
}
struct Delivery(Rc<Desktop>);
impl Drop for Delivery {
    fn drop(&mut self) {
        self.0.in_flight.set(self.0.in_flight.get() - 1);
    }
}
impl Desktop {
    fn connect(self: &Rc<Self>, bus: &gio::DBusConnection) {
        if self.connection.borrow().is_some() {
            return;
        }
        // Subscribe before Notify so token/action/close signals cannot be missed after its reply.
        let weak = Rc::downgrade(self);
        let notifications = bus.signal_subscribe(
            Some(SERVICE),
            Some(SERVICE),
            None,
            Some(PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, signal, parameters| {
                if let Some(desktop) = weak.upgrade() {
                    desktop.signal(signal, parameters);
                }
            },
        );
        let weak = Rc::downgrade(self);
        let owner = bus.signal_subscribe(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            Some(SERVICE),
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, parameters| {
                if let Some((_, old, _)) = parameters.get::<(String, String, String)>() {
                    if old.is_empty() {
                        return;
                    }
                }
                if let Some(desktop) = weak.upgrade() {
                    // A replacement notification server may reuse ids. Never route its clicks to old tabs.
                    desktop
                        .generation
                        .set(desktop.generation.get().wrapping_add(1));
                    desktop.tracked.borrow_mut().clear();
                }
            },
        );
        *self.connection.borrow_mut() = Some(Connection {
            bus: bus.clone(),
            signals: vec![notifications, owner],
        });
    }
    fn signal(&self, signal: &str, parameters: &glib::Variant) {
        if signal == "ActivationToken" {
            if let Some((id, token)) = parameters.get::<(u32, String)>() {
                if let Some(entry) = self
                    .tracked
                    .borrow_mut()
                    .iter_mut()
                    .find(|entry| entry.id == id)
                {
                    entry.token = Some(token);
                }
            }
            return;
        }
        let id = match signal {
            "ActionInvoked" => parameters
                .get::<(u32, String)>()
                .and_then(|(id, action)| (action == "default").then_some(id)),
            "NotificationClosed" => parameters.get::<(u32, u32)>().map(|(id, _)| id),
            _ => None,
        };
        let Some(id) = id else { return };
        let entry = {
            let mut tracked = self.tracked.borrow_mut();
            tracked
                .iter()
                .position(|entry| entry.id == id)
                .and_then(|index| tracked.remove(index))
        };
        // Release state before invoking application code, which can re-enter GTK and delivery.
        if signal == "ActionInvoked" {
            if let Some(entry) = entry {
                (entry.activate)(entry.token);
            }
        }
    }
    pub(in crate::app) fn show(
        self: &Rc<Self>,
        message: Message,
        activate: impl FnOnce(Option<String>) + 'static,
    ) {
        let Some(delivery) = self.reserve() else {
            return;
        };
        let desktop = self.clone();
        gio::bus_get(
            gio::BusType::Session,
            gio::Cancellable::NONE,
            move |result| {
                if let Ok(bus) = result {
                    desktop.send(&bus, message, Box::new(activate), delivery);
                }
            },
        );
    }
    fn reserve(self: &Rc<Self>) -> Option<Delivery> {
        if self.in_flight.get() >= MAX_IN_FLIGHT {
            return None;
        }
        self.in_flight.set(self.in_flight.get() + 1);
        Some(Delivery(self.clone()))
    }
    fn send(
        self: &Rc<Self>,
        bus: &gio::DBusConnection,
        message: Message,
        activate: Activate,
        delivery: Delivery,
    ) {
        self.connect(bus);
        let generation = self.generation.get();
        let actions = if message.focus {
            vec!["default", "Open Worminal"]
        } else {
            vec![]
        };
        let parameters = (
            "Worminal",
            0u32,
            "utilities-terminal",
            message.title,
            glib::markup_escape_text(&message.body).to_string(),
            actions,
            HashMap::<String, glib::Variant>::new(),
            -1i32,
        )
            .to_variant();
        let desktop = self.clone();
        bus.call(
            Some(SERVICE),
            PATH,
            SERVICE,
            "Notify",
            Some(&parameters),
            Some(glib::VariantTy::new("(u)").unwrap()),
            gio::DBusCallFlags::NONE,
            5000,
            gio::Cancellable::NONE,
            move |result| {
                if message.focus && desktop.generation.get() == generation {
                    if let Some((id,)) = result.ok().and_then(|reply| reply.get::<(u32,)>()) {
                        let mut tracked = desktop.tracked.borrow_mut();
                        if id != 0 {
                            tracked.retain(|entry| entry.id != id);
                            if tracked.len() >= MAX_TRACKED {
                                tracked.pop_front();
                            }
                            tracked.push_back(Tracked {
                                id,
                                token: None,
                                activate,
                            });
                        }
                    }
                }
                drop(delivery);
            },
        );
    }
}

#[cfg(test)]
#[path = "notification_desktop_tests.rs"]
mod tests;
