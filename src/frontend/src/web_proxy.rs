//! Keep OpenSSH's SOCKS socket private and give WebKit a stable loopback port.
use glib::translate::ToGlibPtr;
use gtk::{gio, prelude::*};
use std::{ffi::CString, fs, io, path::PathBuf};
use worminal_workspace::ssh;

struct SocketDirectory(PathBuf);
impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_file(self.0.join("socks"));
        let _ = fs::remove_dir(&self.0);
    }
}

pub(crate) struct WebProxy {
    pub uri: CString,
    listener: gio::SocketService,
    cancel: gio::Cancellable,
    _child: ssh::Child,
    _directory: SocketDirectory,
}

impl WebProxy {
    pub fn start(endpoint: &str) -> io::Result<Self> {
        let runtime = glib::user_runtime_dir();
        fs::create_dir_all(&runtime)?;
        let directory = SocketDirectory(
            glib::mkdtemp(runtime.join("worminal-web-XXXXXX"))
                .ok_or_else(io::Error::last_os_error)?,
        );
        let socket = directory.0.join("socks");
        let listener = gio::SocketService::new();
        let bind = gio::InetSocketAddress::new(
            &gio::InetAddress::new_loopback(gio::SocketFamily::Ipv4),
            0,
        );
        let address = listener
            .add_address(
                &bind,
                gio::SocketType::Stream,
                gio::SocketProtocol::Tcp,
                glib::Object::NONE,
            )
            .map_err(io::Error::other)?
            .downcast::<gio::InetSocketAddress>()
            .expect("IPv4 proxy listener");
        let cancel = gio::Cancellable::new();
        let remote = gio::UnixSocketAddress::new(&socket);
        let client = gio::SocketClient::new();
        client.set_enable_proxy(false);
        listener.connect_incoming(glib::clone!(@strong cancel => move |_, incoming, _| {
            let incoming = incoming.clone();
            let splice_cancel = cancel.clone();
            client.connect_async(&remote, Some(&cancel), move |result| {
                if let Ok(outgoing) = result {
                    // GLib owns both streams until copying in either direction ends.
                    let incoming = incoming.upcast::<gio::IOStream>();
                    let outgoing = outgoing.upcast::<gio::IOStream>();
                    unsafe {
                        gio::ffi::g_io_stream_splice_async(
                            incoming.to_glib_none().0,
                            outgoing.to_glib_none().0,
                            gio::ffi::G_IO_STREAM_SPLICE_CLOSE_STREAM1
                                | gio::ffi::G_IO_STREAM_SPLICE_CLOSE_STREAM2,
                            0,
                            splice_cancel.to_glib_none().0,
                            None,
                            std::ptr::null_mut(),
                        );
                    }
                }
            });
            true
        }));

        let child = ssh::dynamic_proxy(endpoint, &socket, &std::env::current_exe()?)?;
        Ok(Self {
            uri: CString::new(format!("socks5://127.0.0.1:{}", address.port()))?,
            listener,
            cancel,
            _child: child,
            _directory: directory,
        })
    }
}

impl Drop for WebProxy {
    fn drop(&mut self) {
        self.listener.stop();
        self.listener.close();
        self.cancel.cancel();
    }
}
