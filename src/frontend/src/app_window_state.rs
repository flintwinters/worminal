//! Read-only session-bus state belongs to the GTK owner, never the service or shell integration.
use super::WINDOWS;
use glib::{prelude::*, Variant};
use gtk::{gio, prelude::*};
use std::{
    cell::{Cell, RefCell},
    ffi::CStr,
    io,
    os::unix::ffi::OsStrExt,
};

mod protocol {
    include!(concat!(env!("OUT_DIR"), "/window_state.rs"));
}
use protocol::*;
fn name(bytes: &'static [u8]) -> &'static str {
    CStr::from_bytes_with_nul(bytes).unwrap().to_str().unwrap()
}
struct Bus {
    connection: gio::DBusConnection,
    registration: Option<gio::RegistrationId>,
    published: Option<Variant>,
}
impl Drop for Bus {
    fn drop(&mut self) {
        if let Some(id) = self.registration.take() {
            let _ = self.connection.unregister_object(id);
        }
    }
}
thread_local! {
    static BUS: RefCell<Option<Bus>> = const { RefCell::new(None) };
    static NEXT_ID: Cell<u32> = const { Cell::new(0) };
}
pub(super) fn next_id() -> io::Result<u32> {
    NEXT_ID.with(|next| {
        let id = next
            .get()
            .checked_add(1)
            .ok_or_else(|| io::Error::other("window ids exhausted"))?;
        next.set(id);
        Ok(id)
    })
}
fn snapshot() -> Variant {
    let rows: Vec<(u32, bool, bool, Vec<u8>, String)> = WINDOWS.with(|windows| {
        windows
            .borrow()
            .iter()
            .filter(|app| !app.closing.get())
            .map(|app| {
                let pages = app.pages.borrow();
                let page = pages.iter().find(|page| page.id == app.active.get());
                let available = app.connection.borrow().is_some()
                    && !app.initial_claim.get()
                    && page.is_some_and(|page| !page.browser && page.directory.is_absolute());
                let directory = if available {
                    page.unwrap().directory.as_os_str().as_bytes().to_vec()
                } else {
                    Vec::new()
                };
                (
                    app.state_id,
                    app.window.is_active(),
                    available,
                    directory,
                    app.config
                        .master
                        .as_ref()
                        .map(|master| master.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect()
    });
    let snapshot = rows.to_variant();
    // Check the D-Bus result type even when the window list is empty.
    assert_eq!(snapshot.type_().as_str(), name(WORMINAL_STATE_WINDOWS));
    Variant::tuple_from_iter([snapshot])
}
pub(super) fn changed() {
    if BUS.with(|slot| slot.borrow().is_none()) {
        return;
    }
    let snapshot = snapshot();
    BUS.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(bus) = slot.as_mut() else {
            return;
        };
        if bus.published.as_ref() != Some(&snapshot) {
            if bus
                .connection
                .emit_signal(
                    None,
                    name(WORMINAL_STATE_PATH),
                    name(WORMINAL_STATE_INTERFACE),
                    name(WORMINAL_STATE_SIGNAL),
                    Some(&snapshot),
                )
                .is_ok()
            {
                bus.published = Some(snapshot);
            }
        }
    });
}
pub(super) struct Owner(Option<gio::OwnerId>);
impl Drop for Owner {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            gio::bus_unown_name(owner);
        }
        BUS.with(|slot| slot.borrow_mut().take());
    }
}
pub(super) fn start() -> Owner {
    let instance = format!("{}{}", name(WORMINAL_STATE_PREFIX), std::process::id());
    // GIO invokes these closures in the initiating GTK thread's default main context.
    // They capture no GTK objects; thread-local state is accessed only in that context.
    Owner(Some(gio::bus_own_name(
        gio::BusType::Session,
        &instance,
        gio::BusNameOwnerFlags::NONE,
        |connection, _| {
            let info = gio::DBusNodeInfo::for_xml(name(WORMINAL_STATE_XML))
                .expect("window-state introspection");
            let interface = info
                .lookup_interface(name(WORMINAL_STATE_INTERFACE))
                .expect("window-state interface");
            let result = connection.register_object(
                name(WORMINAL_STATE_PATH),
                &interface,
                |_, _, _, _, method, _, invocation| {
                    if method == name(WORMINAL_STATE_METHOD) {
                        invocation.return_value(Some(&snapshot()));
                    } else {
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            "Unknown method",
                        );
                    }
                },
                |_, _, _, _, _| false.to_variant(),
                |_, _, _, _, _, _| false,
            );
            if let Ok(registration) = result {
                BUS.with(|slot| {
                    *slot.borrow_mut() = Some(Bus {
                        connection,
                        registration: Some(registration),
                        published: None,
                    })
                });
                changed();
            }
        },
        |_, _| {},
        |_, _| {
            BUS.with(|slot| slot.borrow_mut().take());
        },
    )))
}
