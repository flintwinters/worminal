//! Exercise the real async Notify call and D-Bus signals on a project-local private bus.
use super::*;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct Bus {
    child: Child,
    directory: std::path::PathBuf,
}
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl Bus {
    fn start() -> (Self, String) {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap()
            .join("build/checks")
            .join(format!("notification-bus-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let mut child = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!(
                "--address=unix:path={}",
                directory.join("bus").display()
            ))
            .stdout(Stdio::piped())
            .spawn()
            .expect("private notification test needs dbus-daemon");
        let mut address = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        (Self { child, directory }, address.trim().to_owned())
    }
}
fn connect(address: &str) -> gio::DBusConnection {
    gio::DBusConnection::for_address_sync(
        address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
        gio::Cancellable::NONE,
    )
    .unwrap()
}
fn wait(context: &glib::MainContext, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "private notification service timed out"
        );
        while context.pending() {
            context.iteration(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn name(bus: &gio::DBusConnection, method: &str) {
    let parameters = if method == "RequestName" {
        (SERVICE, 0u32).to_variant()
    } else {
        (SERVICE,).to_variant()
    };
    bus.call_sync(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        method,
        Some(&parameters),
        None,
        gio::DBusCallFlags::NONE,
        5000,
        gio::Cancellable::NONE,
    )
    .unwrap();
}
#[test]
fn desktop_clicks_route_once_with_tokens_and_release_closed_or_stale_targets() {
    let context = glib::MainContext::new();
    context.with_thread_default(|| {
        let (_private, address) = Bus::start();
        let server = connect(&address);
        let client = connect(&address);
        name(&server, "RequestName");
        let info = gio::DBusNodeInfo::for_xml(concat!(
            "<node><interface name='org.freedesktop.Notifications'><method name='Notify'>",
            "<arg type='s' direction='in'/><arg type='u' direction='in'/><arg type='s' direction='in'/>",
            "<arg type='s' direction='in'/><arg type='s' direction='in'/><arg type='as' direction='in'/>",
            "<arg type='a{sv}' direction='in'/><arg type='i' direction='in'/><arg type='u' direction='out'/>",
            "</method></interface></node>"
        )).unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let requests = received.clone();
        let registration = server.register_object(PATH, &info.interfaces()[0],
            move |_, _, _, _, _, parameters, invocation| {
                let mut requests = requests.lock().unwrap();
                requests.push(parameters);
                invocation.return_value(Some(&(requests.len() as u32,).to_variant()));
            }, |_, _, _, _, _| unreachable!(), |_, _, _, _, _, _| false).unwrap();
        let desktop = Rc::new(Desktop::default());
        let clicked = Rc::new(RefCell::new(Vec::new()));
        let deliver = |title: &str, focus: bool| {
            let clicked = clicked.clone();
            let title = title.to_owned();
            let target = title.clone();
            desktop.send(&client, Message { title, focus, ..Message::default() },
                Box::new(move |token| clicked.borrow_mut().push((target, token))), desktop.reserve().unwrap());
            wait(&context, || desktop.in_flight.get() == 0);
        };
        let emit = |connection: &gio::DBusConnection, signal: &str, parameters: glib::Variant| {
            connection.emit_signal(None, PATH, SERVICE, signal, Some(&parameters)).unwrap();
            connection.flush_sync(gio::Cancellable::NONE).unwrap();
        };
        deliver("first tab", true);
        deliver("second tab", true);
        let first_actions = received.lock().unwrap()[0].child_value(5).get::<Vec<String>>().unwrap();
        assert_eq!(first_actions, ["default", "Open Worminal"]);
        // A foreign bus peer cannot synthesize notification actions for this service.
        emit(&client, "ActionInvoked", (2u32, "default").to_variant());
        emit(&server, "ActivationToken", (2u32, "desktop-token").to_variant());
        emit(&server, "ActionInvoked", (2u32, "other").to_variant());
        emit(&server, "ActionInvoked", (2u32, "default").to_variant());
        wait(&context, || !clicked.borrow().is_empty());
        assert_eq!(*clicked.borrow(), [("second tab".into(), Some("desktop-token".into()))]);
        emit(&server, "ActionInvoked", (2u32, "default").to_variant());
        emit(&server, "NotificationClosed", (1u32, 2u32).to_variant());
        wait(&context, || desktop.tracked.borrow().is_empty());
        assert_eq!(clicked.borrow().len(), 1);
        deliver("disabled", false);
        assert!(received.lock().unwrap()[2].child_value(5).get::<Vec<String>>().unwrap().is_empty());
        assert!(desktop.tracked.borrow().is_empty());
        deliver("old server", true);
        name(&server, "ReleaseName");
        wait(&context, || desktop.tracked.borrow().is_empty());
        assert_eq!(clicked.borrow().len(), 1);
        // Retained click callbacks are bounded even if the service never closes notifications.
        name(&server, "RequestName");
        for _ in 0..MAX_TRACKED + 1 { deliver("bounded", true); }
        assert_eq!(desktop.tracked.borrow().len(), MAX_TRACKED);
        assert!(desktop.tracked.borrow().front().unwrap().id > 5);
        let deliveries: Vec<_> = (0..MAX_IN_FLIGHT).map(|_| desktop.reserve().unwrap()).collect();
        assert!(desktop.reserve().is_none());
        drop(deliveries);
        assert_eq!(desktop.in_flight.get(), 0);
        let weak = Rc::downgrade(&desktop);
        drop(desktop);
        assert!(weak.upgrade().is_none(), "subscriptions must not retain their window's desktop owner");
        server.unregister_object(registration).unwrap();
        client.close_sync(gio::Cancellable::NONE).unwrap();
        server.close_sync(gio::Cancellable::NONE).unwrap();
    }).unwrap();
}
