//! In-memory cookie transfer over the frontend's private same-user update channel; never logged or saved.
use glib::translate::*;
use gtk::gio;
use std::{ffi::c_void, ptr};
use webkit2gtk::*;
use worminal_browser_api::WorminalBrowserProfileReply;
use worminal_protocol::{Buffer, Payload, MAX_PAYLOAD};

type Async = unsafe extern "C" fn(
    *mut c_void,
    *mut gio::ffi::GCancellable,
    Option<
        unsafe extern "C" fn(
            *mut glib::gobject_ffi::GObject,
            *mut gio::ffi::GAsyncResult,
            *mut c_void,
        ),
    >,
    *mut c_void,
);
type ExportFinish = unsafe extern "C" fn(
    *mut c_void,
    *mut gio::ffi::GAsyncResult,
    *mut *mut glib::ffi::GError,
) -> *mut glib::ffi::GList;
type Import = unsafe extern "C" fn(
    *mut c_void,
    *mut glib::ffi::GList,
    *mut gio::ffi::GCancellable,
    Option<
        unsafe extern "C" fn(
            *mut glib::gobject_ffi::GObject,
            *mut gio::ffi::GAsyncResult,
            *mut c_void,
        ),
    >,
    *mut c_void,
);
type ImportFinish = unsafe extern "C" fn(
    *mut c_void,
    *mut gio::ffi::GAsyncResult,
    *mut *mut glib::ffi::GError,
) -> i32;

struct Transfer {
    _profile: WebsiteDataManager,
    manager: CookieManager,
    callback: WorminalBrowserProfileReply,
    context: *mut c_void,
    export_finish: Option<ExportFinish>,
    import_finish: Option<ImportFinish>,
    _cookies: Vec<soup::Cookie>,
    list: *mut glib::ffi::GList,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        unsafe {
            glib::ffi::g_list_free(self.list);
        }
    }
}
impl Transfer {
    fn reply(&self, result: Result<Vec<u8>, ()>) {
        if let Some(callback) = self.callback {
            let (status, bytes) = match &result {
                Ok(bytes) => (0, bytes.as_slice()),
                Err(()) => (1, &[][..]),
            };
            unsafe {
                callback(self.context, status, bytes.as_ptr(), bytes.len() as u64);
            }
        }
    }
}
unsafe extern "C" fn completed(
    _: *mut glib::gobject_ffi::GObject,
    result: *mut gio::ffi::GAsyncResult,
    data: *mut c_void,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        completed_inner(result, data);
    }));
}
unsafe fn completed_inner(result: *mut gio::ffi::GAsyncResult, data: *mut c_void) {
    let transfer = unsafe { Box::from_raw(data.cast::<Transfer>()) };
    let mut error = ptr::null_mut();
    let manager: *mut webkit2gtk::ffi::WebKitCookieManager = transfer.manager.to_glib_none().0;
    let reply = if let Some(finish) = transfer.export_finish {
        let list = unsafe { finish(manager.cast(), result, &mut error) };
        if error.is_null() {
            let mut cookies: Vec<soup::Cookie> =
                unsafe { FromGlibPtrContainer::from_glib_full(list) };
            let mut body = Buffer::default();
            (|| -> Result<Vec<u8>, ()> {
                body.put_u32(cookies.len().try_into().map_err(|_| ())?)
                    .map_err(|_| ())?;
                for cookie in &mut cookies {
                    let header = cookie.to_set_cookie_header().ok_or(())?;
                    body.put_string(header.as_bytes()).map_err(|_| ())?;
                    body.put_string(cookie.domain().ok_or(())?.as_bytes())
                        .map_err(|_| ())?;
                    body.put_u32(cookie.same_site_policy().into_glib() as u32)
                        .map_err(|_| ())?;
                }
                Ok(body.into_bytes())
            })()
        } else {
            Err(())
        }
    } else {
        let success =
            unsafe { transfer.import_finish.unwrap()(manager.cast(), result, &mut error) };
        if success != 0 && error.is_null() {
            Ok(Vec::new())
        } else {
            Err(())
        }
    };
    if !error.is_null() {
        unsafe {
            glib::ffi::g_error_free(error);
        }
    }
    transfer.reply(reply);
}

#[no_mangle]
pub unsafe extern "C" fn worminal_browser_profile_transfer_v1(
    profile: *mut c_void,
    bytes: *const u8,
    length: u64,
    callback: WorminalBrowserProfileReply,
    context: *mut c_void,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        transfer(profile, bytes, length, callback, context)
    }))
    .unwrap_or(0)
}
unsafe fn transfer(
    profile: *mut c_void,
    bytes: *const u8,
    length: u64,
    callback: WorminalBrowserProfileReply,
    context: *mut c_void,
) -> i32 {
    if profile.is_null() || callback.is_none() || length > MAX_PAYLOAD as u64 {
        return 0;
    }
    let exporting = bytes.is_null();
    let start = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            if exporting {
                c"webkit_cookie_manager_get_all_cookies".as_ptr()
            } else {
                c"webkit_cookie_manager_replace_cookies".as_ptr()
            },
        )
    };
    let finish = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            if exporting {
                c"webkit_cookie_manager_get_all_cookies_finish".as_ptr()
            } else {
                c"webkit_cookie_manager_replace_cookies_finish".as_ptr()
            },
        )
    };
    if start.is_null() || finish.is_null() {
        return 0;
    }
    let profile: WebsiteDataManager = unsafe { from_glib_none(profile.cast()) };
    let Some(manager) = profile.cookie_manager() else {
        return 0;
    };
    let mut cookies = Vec::new();
    let mut list = ptr::null_mut();
    if !exporting {
        let input = unsafe { std::slice::from_raw_parts(bytes, length as usize) };
        let parsed = (|| -> Result<(), ()> {
            let mut input = Payload::new(input);
            let count = input.u32().map_err(|_| ())?;
            if count > 65536 {
                return Err(());
            }
            for _ in 0..count {
                let header =
                    std::str::from_utf8(input.string().map_err(|_| ())?).map_err(|_| ())?;
                let domain =
                    std::str::from_utf8(input.string().map_err(|_| ())?).map_err(|_| ())?;
                let policy = input.u32().map_err(|_| ())?;
                if policy > 2 {
                    return Err(());
                }
                let mut cookie = soup::Cookie::parse(header, None).ok_or(())?;
                cookie.set_domain(domain);
                cookie.set_same_site_policy(unsafe { from_glib(policy as i32) });
                cookies.push(cookie);
            }
            input.finish().map_err(|_| ())
        })();
        if parsed.is_err() {
            return 0;
        }
        for cookie in &mut cookies {
            list = unsafe { glib::ffi::g_list_append(list, cookie.to_glib_none_mut().0.cast()) };
        }
    }
    let transfer = Box::new(Transfer {
        _profile: profile,
        manager: manager.clone(),
        callback,
        context,
        export_finish: exporting
            .then(|| unsafe { std::mem::transmute::<*mut c_void, ExportFinish>(finish) }),
        import_finish: (!exporting)
            .then(|| unsafe { std::mem::transmute::<*mut c_void, ImportFinish>(finish) }),
        _cookies: cookies,
        list,
    });
    let data = Box::into_raw(transfer).cast();
    let manager: *mut webkit2gtk::ffi::WebKitCookieManager = manager.to_glib_none().0;
    if exporting {
        let start: Async = unsafe { std::mem::transmute(start) };
        unsafe {
            start(manager.cast(), ptr::null_mut(), Some(completed), data);
        }
    } else {
        let start: Import = unsafe { std::mem::transmute(start) };
        unsafe {
            start(manager.cast(), list, ptr::null_mut(), Some(completed), data);
        }
    }
    1
}
