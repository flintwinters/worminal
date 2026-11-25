//! Certificate exceptions persist in the browser profile; the user's choice is final.
use gtk::{gio, prelude::*};
use std::{fs, io::Write, path::PathBuf};
use webkit2gtk::{WebContext, WebContextExt, WebsiteDataManager, WebsiteDataManagerExt};

/// One `host<TAB>pem` record per line; PEM newlines are stored as `\n` escapes (PEM has no backslash).
fn store(profile: &WebsiteDataManager) -> Option<PathBuf> {
    Some(PathBuf::from(profile.base_data_directory()?).join("trusted-certificates"))
}
/// Replays saved exceptions into a fresh context. Unreadable records are skipped.
pub(crate) fn load(profile: &WebsiteDataManager, context: &WebContext) {
    let Some(records) = store(profile).and_then(|path| fs::read_to_string(path).ok()) else {
        return;
    };
    for (host, pem) in records.lines().filter_map(|line| line.split_once('\t')) {
        if let Ok(certificate) = gio::TlsCertificate::from_pem(&pem.replace("\\n", "\n")) {
            context.allow_tls_certificate_for_host(&certificate, host);
        }
    }
}
/// Trusts `certificate` for `host` now and in every later session. Ephemeral profiles have no
/// store, so their exceptions last as long as the context.
pub(crate) fn allow(
    profile: &WebsiteDataManager,
    context: &WebContext,
    certificate: &gio::TlsCertificate,
    host: &str,
) {
    context.allow_tls_certificate_for_host(certificate, host);
    let Some(path) = store(profile) else { return };
    let pem = certificate
        .certificate_pem()
        .unwrap_or_default()
        .replace('\n', "\\n");
    let appended = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{host}\t{pem}"));
    if let Err(error) = appended {
        eprintln!(
            "worminal: cannot save certificate exception {}: {error}",
            path.display()
        );
    }
}
