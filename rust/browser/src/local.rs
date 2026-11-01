//! Capabilities belong to the initially selected top-level local document, including query routes.
use glib::{Uri, UriFlags, UriHideFlags};
use gtk::{gio, prelude::*};
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::OpenOptions,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::Path,
};
use webkit2gtk::*;

struct Capability {
    view: glib::WeakRef<WebView>,
    document: String,
    credential: String,
}
thread_local! { static CAPABILITIES: RefCell<Vec<Capability>> = const { RefCell::new(Vec::new()) }; }

pub(crate) fn document(url: &str) -> Option<String> {
    let uri = Uri::parse(url, UriFlags::ENCODED).ok()?;
    (uri.scheme().as_str() == "file"
        && uri
            .host()
            .is_none_or(|host| host.is_empty() || host.eq_ignore_ascii_case("localhost")))
    .then(|| {
        uri.to_string_partial(UriHideFlags::QUERY | UriHideFlags::FRAGMENT)
            .to_string()
    })
}
pub(crate) fn authorize(view: &WebView, manager: &UserContentManager, url: &str) {
    let Some(document) = document(url) else {
        return;
    };
    let credential = glib::uuid_string_random().to_string();
    let escaped: String = document
        .chars()
        .map(|value| match value {
            '\\' => "\\\\".into(),
            '"' => "\\\"".into(),
            value if value.is_control() || matches!(value, '\u{2028}' | '\u{2029}') => {
                format!("\\u{:04x}", value as u32)
            }
            value => value.to_string(),
        })
        .collect();
    let source = include_str!("../../../src/gtk/browser-local.js")
        .replacen("%s", &escaped, 1)
        .replacen("%s", &credential, 1);
    manager.add_script(&UserScript::new(
        &source,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::Start,
        &[],
        &[],
    ));
    CAPABILITIES.with(|caps| {
        let mut caps = caps.borrow_mut();
        caps.retain(|cap| cap.view.upgrade().is_some());
        caps.push(Capability {
            view: view.downgrade(),
            document,
            credential,
        });
    });
}
fn error(code: gio::IOErrorEnum, message: &str) -> glib::Error {
    glib::Error::new(code, message)
}
fn denied() -> glib::Error {
    error(
        gio::IOErrorEnum::PermissionDenied,
        "Local access requires the selected local document",
    )
}
fn form(query: &str) -> Result<HashMap<Vec<u8>, Vec<u8>>, glib::Error> {
    fn decode(text: &str) -> Option<Vec<u8>> {
        let mut output = Vec::new();
        let mut input = text.as_bytes().iter().copied();
        while let Some(byte) = input.next() {
            output.push(match byte {
                b'+' => b' ',
                b'%' => {
                    (input.next()? as char).to_digit(16)? as u8 * 16
                        + (input.next()? as char).to_digit(16)? as u8
                }
                value => value,
            });
        }
        (!output.contains(&0)).then_some(output)
    }
    query
        .split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((
                decode(key).ok_or_else(denied)?,
                decode(value).ok_or_else(denied)?,
            ))
        })
        .collect()
}
fn response(request: &URISchemeRequest, stream: &gio::InputStream, length: i64, kind: &str) {
    let response = URISchemeResponse::new(stream, length);
    response.set_content_type(kind);
    let headers = soup::MessageHeaders::new(soup::MessageHeadersType::Response);
    headers.append("Access-Control-Allow-Origin", "null");
    response.set_http_headers(headers);
    request.finish_with_response(&response);
}
fn serve(request: &URISchemeRequest) -> Result<(), glib::Error> {
    let uri = Uri::parse(&request.uri().ok_or_else(denied)?, UriFlags::ENCODED_QUERY)?;
    if uri.host().is_some_and(|host| !host.is_empty())
        || !matches!(uri.path().as_str(), "/file" | "/list")
    {
        return Err(denied());
    }
    let params = form(uri.query().as_deref().unwrap_or(""))?;
    let view = request.web_view().ok_or_else(denied)?;
    let top = view.uri().and_then(|uri| document(&uri));
    let allowed = CAPABILITIES.with(|caps| {
        caps.borrow().iter().any(|cap| {
            cap.view.upgrade().as_ref() == Some(&view)
                && top.as_deref() == Some(&cap.document)
                && params
                    .get(b"credential".as_slice())
                    .is_some_and(|value| value == cap.credential.as_bytes())
        })
    });
    if !allowed {
        return Err(denied());
    }
    let path = params.get(b"path".as_slice()).ok_or_else(denied)?;
    let path = Path::new(std::ffi::OsStr::from_bytes(path));
    if !path.is_absolute() {
        return Err(error(
            gio::IOErrorEnum::InvalidArgument,
            "Local path must be absolute",
        ));
    }
    if uri.path().as_str() == "/file" {
        // Inspect the descriptor returned by nonblocking open, never a racing prior stat; FIFOs cannot stall GTK.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
            .open(path)
            .map_err(|_| error(gio::IOErrorEnum::Failed, "Cannot open local file"))?;
        let info = file
            .metadata()
            .map_err(|_| error(gio::IOErrorEnum::Failed, "Cannot inspect local file"))?;
        if !info.is_file() || info.len() > 32 * 1024 * 1024 {
            return Err(error(
                gio::IOErrorEnum::Failed,
                "Preview requires a regular file no larger than 32 MiB",
            ));
        }
        response(
            request,
            &crate::boundary::input(file),
            info.len() as i64,
            "application/octet-stream",
        );
    } else {
        let items = gio::File::for_path(path).enumerate_children(
            "standard::name,standard::type,standard::size,time::modified,time::modified-usec",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )?;
        let mut json = String::from("[");
        while let Some(info) = items.next_file(gio::Cancellable::NONE)? {
            if json.len() > 1 {
                json.push(',');
            }
            json.push_str(&format!(
                "[\"{}\",{},{},{}]",
                glib::base64_encode(info.name().as_os_str().as_bytes()),
                info.file_type() == gio::FileType::Directory,
                info.size(),
                info.attribute_uint64("time::modified") * 1000
                    + u64::from(info.attribute_uint32("time::modified-usec")) / 1000
            ));
        }
        json.push(']');
        let bytes = glib::Bytes::from_owned(json.into_bytes());
        response(
            request,
            &gio::MemoryInputStream::from_bytes(&bytes).upcast(),
            bytes.len() as i64,
            "application/json",
        );
    }
    Ok(())
}
pub(crate) fn register(context: &WebContext) {
    let scheme = std::ffi::CStr::from_bytes_with_nul(worminal_workspace::WORKSPACE_BROWSER_SCHEME)
        .unwrap()
        .to_str()
        .unwrap();
    context.register_uri_scheme(scheme, |request| {
        if let Err(mut error) = serve(request) {
            request.finish_error(&mut error);
        }
    });
    context
        .security_manager()
        .expect("security manager")
        .register_uri_scheme_as_cors_enabled(scheme);
}
