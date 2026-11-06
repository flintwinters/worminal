//! Navigation, permission and download decisions remain native and explicit.
use crate::Browser;
use gtk::prelude::*;
use std::rc::Rc;
use webkit2gtk::*;
use worminal_browser_api::*;

pub(crate) fn connect(browser: &Rc<Browser>) {
    browser.view.connect_decide_policy(glib::clone!(@weak browser => @default-return true, move |_, decision, kind| {
        if kind == PolicyDecisionType::Response {
            if let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() {
                if !response.is_mime_type_supported() { decision.download(); return true; }
            }
        } else if matches!(kind, PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction) {
            if let Some(navigation) = decision.downcast_ref::<NavigationPolicyDecision>() {
                if let Some(action) = navigation.navigation_action() {
                    let uri = action.request().and_then(|request| request.uri()).unwrap_or_default();
                    if kind == PolicyDecisionType::NewWindowAction {
                        if action.is_user_gesture() { browser.emit(WORMINAL_BROWSER_OPEN, &uri); }
                        decision.ignore(); return true;
                    }
                    let scheme = glib::Uri::parse_scheme(&uri).map(|scheme| scheme.to_ascii_lowercase());
                    let local = worminal_workspace::WORKSPACE_BROWSER_SCHEME;
                    if !matches!(scheme.as_deref(), Some("http" | "https" | "about" | "file" | "data" | "blob"))
                        && scheme.as_deref() != Some(local) {
                        decision.ignore(); browser.notice(Some("This URL scheme is unsupported.")); return true;
                    }
                }
            }
        }
        false
    }));
    browser.view.connect_create(
        glib::clone!(@weak browser => @default-return None, move |_, action| {
            if action.is_user_gesture() {
                if let Some(uri) = action.request().and_then(|request| request.uri()) {
                    browser.emit(WORMINAL_BROWSER_OPEN, &uri);
                }
            }
            None
        }),
    );
    browser.view.connect_close(
        glib::clone!(@weak browser => move |_| browser.emit(WORMINAL_BROWSER_CLOSE, "")),
    );
    browser.view.connect_enter_fullscreen(
        glib::clone!(@weak browser => @default-return true, move |_| {
            browser.notice(Some("Page fullscreen requests are unsupported.")); true
        }),
    );
    browser.view.connect_permission_request(glib::clone!(@weak browser => @default-return true, move |view, request| {
        let what = if request.is::<GeolocationPermissionRequest>() { "your location" }
            else if request.is::<UserMediaPermissionRequest>() { "your camera or microphone" }
            else if request.is::<NotificationPermissionRequest>() { "desktop notifications" }
            else { request.deny(); return true; };
        let parent = view.toplevel().and_then(|widget| widget.downcast::<gtk::Window>().ok());
        let dialog = gtk::MessageDialog::new(parent.as_ref(), gtk::DialogFlags::DESTROY_WITH_PARENT,
            gtk::MessageType::Question, gtk::ButtonsType::None,
            &format!("Allow {} to access {what}?", view.uri().unwrap_or_default()));
        dialog.add_buttons(&[("Deny", gtk::ResponseType::Cancel), ("Allow", gtk::ResponseType::Accept)]);
        dialog.set_default_response(gtk::ResponseType::Cancel);
        // A response or parent destruction answers this request exactly once.
        let answered = Rc::new(std::cell::Cell::new(false));
        dialog.connect_response(glib::clone!(@strong request, @strong answered => move |dialog, response| {
            if !answered.replace(true) {
                if response == gtk::ResponseType::Accept { request.allow(); } else { request.deny(); }
            }
            dialog.close();
        }));
        dialog.connect_destroy(glib::clone!(@strong request => move |_| {
            if !answered.replace(true) { request.deny(); }
        }));
        dialog.show(); true
    }));
}
pub(crate) fn downloads(context: &WebContext) {
    context.connect_download_started(|_, download| {
        download.connect_finished(|download| {
            if std::env::var_os("WORMINAL_TRACE_TIMING").is_some() {
                eprintln!("worminal-download finished {:?}", download.destination());
            }
        });
        download.connect_failed(|_, error| {
            if !error.matches(DownloadError::CancelledByUser) {
                eprintln!("worminal: download failed: {error}");
            }
        });
        download.connect_decide_destination(|download, suggestion| {
            let parent = download
                .web_view()
                .and_then(|view| view.toplevel())
                .and_then(|widget| widget.downcast::<gtk::Window>().ok());
            let dialog = gtk::FileChooserNative::new(
                Some("Save download"),
                parent.as_ref(),
                gtk::FileChooserAction::Save,
                Some("Save"),
                Some("Cancel"),
            );
            let name = std::path::Path::new(suggestion)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            dialog.set_current_name(&name);
            dialog.set_do_overwrite_confirmation(true);
            // Native dialogs are not GTK toplevel widgets. Keep this reference until response
            // or owner destruction, then break the keeper/signal cycle in the answering operation.
            let keeper = Rc::new(std::cell::RefCell::new(Some(dialog.clone())));
            dialog.connect_response(
                glib::clone!(@strong download, @strong keeper => move |dialog, response| {
                    if std::env::var_os("WORMINAL_TRACE_TIMING").is_some() {
                        eprintln!("worminal-download response {response:?} {:?}", dialog.uri());
                    }
                    if response == gtk::ResponseType::Accept {
                        if let Some(uri) = dialog.uri() {
                            download.set_allow_overwrite(true); download.set_destination(&uri);
                        }
                        else { download.cancel(); }
                    } else { download.cancel(); }
                    dialog.destroy();
                    keeper.borrow_mut().take();
                }),
            );
            if let Some(parent) = parent {
                parent.connect_destroy(glib::clone!(@weak dialog, @strong download => move |_| {
                    download.cancel(); dialog.destroy(); keeper.borrow_mut().take();
                }));
            }
            dialog.show();
            true
        });
    });
}
