use std::{fs::File, os::unix::net::UnixStream};
use worminal_workspace::{
    broker::Request,
    gui_handoff::{self, Window},
};

fn window(selected: u32) -> Window {
    Window {
        request: Request {
            args: vec!["worminal".into(), "--attach".into()],
            directory: "/".into(),
            environment: vec!["FIXTURE=present".into()],
            output: File::open("/dev/null").unwrap(),
        },
        selected,
        geometry: [-40, 60, 800, 500],
        flags: 4,
        zoom: 1.5,
    }
}

#[test]
fn transfers_multiple_windows_and_descriptor_ownership() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener};
    let name = format!("worminal-handoff-test-{}", std::process::id());
    let listener =
        UnixListener::bind_addr(&SocketAddr::from_abstract_name(name.as_bytes()).unwrap()).unwrap();
    let lease = File::open("/dev/null").unwrap();
    let (mut sender, mut receiver) = UnixStream::pair().unwrap();
    let state = [window(7), window(11)];
    let expected = gui_handoff::encode(&state).unwrap();
    gui_handoff::send(&mut sender, &listener, &lease, &state).unwrap();
    let (received_listener, received_lease, received) =
        gui_handoff::receive(&mut receiver).unwrap();
    drop((listener, lease, state));
    assert_eq!(gui_handoff::encode(&received).unwrap(), expected);
    assert_eq!(
        received_listener.local_addr().unwrap().as_abstract_name(),
        Some(name.as_bytes())
    );
    assert!(received_lease.metadata().is_ok());
    assert!(received
        .iter()
        .all(|window| window.request.output.metadata().is_ok()));
}

#[test]
fn rejects_empty_and_invalid_geometry_and_zoom() {
    assert!(gui_handoff::encode(&[]).is_err());
    let mut state = window(1);
    state.geometry[2] = 0;
    assert!(gui_handoff::encode(&[state]).is_err());
    let mut state = window(1);
    state.zoom = f64::NAN;
    assert!(gui_handoff::encode(&[state]).is_err());
}
