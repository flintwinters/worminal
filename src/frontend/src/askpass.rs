//! OpenSSH prompt helper. Runs in its own process so authentication never blocks the owner.
use gtk::prelude::*;
use std::{
    ffi::OsStr,
    io::{self, Write},
};

pub(crate) fn run(prompt: &OsStr) -> io::Result<()> {
    gtk::init().map_err(io::Error::other)?;
    let confirm = std::env::var_os("SSH_ASKPASS_PROMPT").as_deref() == Some(OsStr::new("confirm"));
    let dialog = gtk::Dialog::with_buttons(
        Some("Worminal SSH authentication"),
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
        &[
            ("Cancel", gtk::ResponseType::Cancel),
            (
                if confirm { "Continue" } else { "Connect" },
                gtk::ResponseType::Ok,
            ),
        ],
    );
    dialog.set_default_response(gtk::ResponseType::Ok);
    dialog.set_border_width(12);
    let label = gtk::Label::new(Some(&prompt.to_string_lossy()));
    label.set_line_wrap(true);
    label.set_max_width_chars(72);
    dialog.content_area().set_spacing(12);
    dialog.content_area().add(&label);
    let entry = gtk::Entry::new();
    entry.set_visibility(false);
    entry.set_activates_default(true);
    if !confirm {
        dialog.content_area().add(&entry);
    }
    dialog.show_all();
    entry.grab_focus();
    let accepted = dialog.run() == gtk::ResponseType::Ok;
    let answer = entry.text();
    entry.set_text("");
    dialog.close();
    if !accepted {
        return Err(io::Error::other("SSH authentication cancelled"));
    }
    // Only OpenSSH's private helper pipe receives the response; never log credentials.
    let mut output = io::stdout().lock();
    writeln!(output, "{}", if confirm { "yes" } else { answer.as_str() })?;
    output.flush()
}
