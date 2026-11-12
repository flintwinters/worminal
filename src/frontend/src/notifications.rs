//! Bounded OSC 99 title/body assembly. Desktop delivery belongs to the view, including over SSH.
use gtk::glib;
use std::collections::HashMap;
#[path = "notification_desktop.rs"]
mod desktop;
pub(super) use desktop::Desktop;

const MAX_TEXT: usize = 16 * 1024;
const MAX_PENDING: usize = 32;

#[derive(Default)]
pub(super) struct Notifications {
    pending: HashMap<(u32, String), Message>,
}
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Message {
    title: String,
    body: String,
    focus: bool,
}
impl Default for Message {
    fn default() -> Self {
        Self {
            title: String::new(),
            body: String::new(),
            focus: true,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Show(Message),
    Reply(Vec<u8>),
}
impl Notifications {
    pub(super) fn receive(&mut self, tab: u32, bytes: &[u8]) -> Option<Outcome> {
        let text = std::str::from_utf8(bytes).ok()?;
        let (metadata, payload) = text.split_once(';')?;
        let (mut id, mut kind, mut done, mut encoded) = ("", "title", true, false);
        let mut actions = None;
        for field in metadata.split(':') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "i" => id = value,
                "p" => kind = value,
                "d" => done = value != "0",
                "e" => encoded = value == "1",
                "a" => actions = Some(value),
                _ => {}
            }
        }
        // Identifiers are reflected into terminal replies and must remain escape-code safe.
        if id.len() > 512
            || !id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_+.".contains(&c))
        {
            return None;
        }
        if kind == "?" {
            return Some(Outcome::Reply(
                format!("\x1b]99;i={id}:p=?;p=title,body:o=always:a=focus\x1b\\").into_bytes(),
            ));
        }
        if !matches!(kind, "title" | "body") {
            return None;
        }
        let payload = if encoded {
            // GLib's decoder is permissive; reject malformed input before using it.
            let bytes = glib::base64_decode(payload);
            if glib::base64_encode(&bytes) != payload {
                return None;
            }
            String::from_utf8(bytes).ok()?
        } else {
            payload.to_owned()
        };
        if payload.chars().any(char::is_control) {
            return None;
        }
        let key = (tab, id.to_owned());
        if !self.pending.contains_key(&key) && self.pending.len() >= MAX_PENDING {
            return None;
        }
        let message = self.pending.entry(key.clone()).or_default();
        if let Some(actions) = actions {
            for action in actions.split(',') {
                match action {
                    "focus" => message.focus = true,
                    "-focus" => message.focus = false,
                    _ => {}
                }
            }
        }
        if message.title.len() + message.body.len() + payload.len() > MAX_TEXT {
            self.pending.remove(&key);
            return None;
        }
        if kind == "body" {
            message.body.push_str(&payload);
        } else {
            message.title.push_str(&payload);
        }
        if !done {
            return None;
        }
        let mut message = self.pending.remove(&key)?;
        if message.title.is_empty() {
            message.title = std::mem::take(&mut message.body);
        }
        (!message.title.is_empty()).then_some(Outcome::Show(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chunks_preserve_payload_and_are_isolated_by_tab_and_id() {
        let mut notifications = Notifications::default();
        assert_eq!(notifications.receive(1, b"i=x:d=0;Build; "), None);
        assert_eq!(notifications.receive(2, b"i=x:d=0;Other"), None);
        assert_eq!(notifications.receive(1, b"i=x:d=0;done"), None);
        assert_eq!(
            notifications.receive(1, b"i=x:p=body:e=1;SGVsbG8="),
            Some(Outcome::Show(Message {
                title: "Build; done".into(),
                body: "Hello".into(),
                ..Message::default()
            }))
        );
        assert_eq!(
            notifications.receive(2, b"i=x;p"),
            Some(Outcome::Show(Message {
                title: "Otherp".into(),
                body: String::new(),
                ..Message::default()
            }))
        );
    }
    #[test]
    fn query_unknown_types_and_invalid_data_do_not_notify() {
        let mut n = Notifications::default();
        assert_eq!(
            n.receive(1, b"i=q:p=?;"),
            Some(Outcome::Reply(
                b"\x1b]99;i=q:p=?;p=title,body:o=always:a=focus\x1b\\".to_vec()
            ))
        );
        for bytes in [
            b"p=icon;image".as_slice(),
            b"e=1;???",
            b";",
            b"i=bad\x1b;p",
            b";\xff",
        ] {
            assert_eq!(n.receive(1, bytes), None);
        }
        assert_eq!(
            n.receive(1, b"p=body;Body only"),
            Some(Outcome::Show(Message {
                title: "Body only".into(),
                body: String::new(),
                ..Message::default()
            }))
        );
    }
    #[test]
    fn focus_defaults_on_and_can_be_disabled_across_chunks() {
        let mut n = Notifications::default();
        assert_eq!(n.receive(1, b"i=x:d=0:a=-focus;title"), None);
        assert_eq!(
            n.receive(1, b"i=x:p=body;body"),
            Some(Outcome::Show(Message {
                title: "title".into(),
                body: "body".into(),
                focus: false
            }))
        );
        assert_eq!(
            n.receive(1, b"a=-focus,focus;title"),
            Some(Outcome::Show(Message {
                title: "title".into(),
                ..Message::default()
            }))
        );
    }
    #[test]
    fn pending_storage_is_bounded() {
        let mut n = Notifications::default();
        for id in 0..100 {
            n.receive(1, format!("i={id}:d=0;title").as_bytes());
        }
        assert_eq!(n.pending.len(), MAX_PENDING);
        n.receive(1, format!("i=0:d=0;{}", "x".repeat(MAX_TEXT)).as_bytes());
        assert!(!n.pending.contains_key(&(1, "0".into())));
    }
}
