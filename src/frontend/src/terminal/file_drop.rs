//! File-manager drops use the ordinary paste path, with shell-safe local filenames.
use super::*;
use std::os::unix::ffi::OsStrExt;

impl Inner {
    pub(super) fn connect_file_drop(self: &Rc<Self>) {
        self.widget.drag_dest_set(
            gtk::DestDefaults::ALL,
            &[gtk::TargetEntry::new(
                "text/uri-list",
                gtk::TargetFlags::empty(),
                0,
            )],
            gdk::DragAction::COPY,
        );
        self.widget.connect_drag_data_received(
            glib::clone!(@weak self as inner => move |widget, context, _, _, data, _, time| {
                let paths = file_paths(&data.uris());
                let accepted = paths.is_some() && !inner.state.borrow().destroyed;
                if accepted {
                    widget.grab_focus();
                    inner.paste_bytes(paths.unwrap());
                }
                context.drag_finish(accepted, false, time);
            }),
        );
    }
}

fn file_paths(uris: &[glib::GString]) -> Option<Vec<u8>> {
    if uris.is_empty() {
        return None;
    }
    let mut bytes = Vec::new();
    for uri in uris {
        let (path, host) = glib::filename_from_uri(uri).ok()?;
        if !path.is_absolute() || host.is_some_and(|host| !host.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        if !bytes.is_empty() {
            bytes.push(b' ');
        }
        bytes.extend_from_slice(glib::shell_quote(path).as_bytes());
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_quotes_each_absolute_path() {
        let uris =
            ["file:///home/user/a%20b", "file:///home/user/a%27%24x"].map(glib::GString::from);
        assert_eq!(
            file_paths(&uris).unwrap(),
            b"'/home/user/a b' '/home/user/a'\\''$x'"
        );
        assert_eq!(
            file_paths(&[glib::GString::from("file:///home/user/%FF")]).unwrap(),
            b"'/home/user/\xff'"
        );
    }

    #[test]
    fn rejects_nonlocal_or_invalid_drops_as_a_whole() {
        for uri in [
            "https://example.com/a",
            "file://other/home/a",
            "file:relative",
            "file:///bad%00name",
        ] {
            assert!(file_paths(&["file:///valid".into(), uri.into()]).is_none());
        }
        assert!(file_paths(&[]).is_none());
        assert!(file_paths(&["file://localhost/home/a".into()]).is_some());
    }
}
