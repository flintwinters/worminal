//! Load browser modules. Each view keeps the module that created it loaded.
use glib::{prelude::*, translate::*};
use std::{
    cell::RefCell,
    ffi::{c_void, CStr, CString},
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
    rc::Rc,
};
use worminal_browser_api::*;

struct Module {
    api: &'static WorminalRustBrowser,
    identity: (u64, u64),
    _file: File,
}
struct UnpublishedLibrary(*mut c_void);
impl Drop for UnpublishedLibrary {
    fn drop(&mut self) {
        unsafe {
            libc::dlclose(self.0);
        }
    }
}
struct Profile {
    object: glib::Object,
    _lock: File,
}
// Stable product data predates the Rust port. Candidate owners remain isolated until cutover.
const WORMINAL_BROWSER_PROFILE_STABLE: &str = "worminal-preview";
const WORMINAL_BROWSER_PROFILE_CANDIDATE: &str = "worminal-rust";
const WORMINAL_BROWSER_PROFILE_LOCK: &str = "owner.lock";
thread_local! {
    // Code remains resident for framework callbacks after the last view/context is released.
    static MODULES: RefCell<Vec<Rc<Module>>> = const { RefCell::new(Vec::new()) };
    static PROFILE: RefCell<Option<Profile>> = const { RefCell::new(None) };
    static PROFILE_NAME: RefCell<&'static str> = const { RefCell::new(WORMINAL_BROWSER_PROFILE_CANDIDATE) };
}
pub(crate) fn configure_profile(candidate: bool) -> io::Result<()> {
    let name = if candidate {
        WORMINAL_BROWSER_PROFILE_CANDIDATE
    } else {
        WORMINAL_BROWSER_PROFILE_STABLE
    };
    PROFILE_NAME.with(|slot| {
        if *slot.borrow() != name && PROFILE.with(|profile| profile.borrow().is_some()) {
            return Err(io::Error::other("cannot change a live browser profile"));
        }
        *slot.borrow_mut() = name;
        Ok(())
    })
}
type Callback = Rc<dyn Fn(u32, &str)>;
struct Inner {
    module: Rc<Module>,
    handle: *mut c_void,
    widget: gtk::Widget,
    _callback: Box<Callback>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        unsafe {
            (self.module.api.destroy.unwrap())(self.handle);
        }
    }
}
#[derive(Clone)]
pub(crate) struct Browser(Rc<Inner>);

fn module_path() -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("missing executable directory"))?;
    let name = std::ffi::OsStr::from_bytes(
        CStr::from_bytes_with_nul(WORMINAL_RUST_BROWSER_FILE)
            .unwrap()
            .to_bytes(),
    );
    Ok(directory.join("../lib/worminal").join(name))
}
fn loader_error() -> io::Error {
    let pointer = unsafe { libc::dlerror() };
    io::Error::other(if pointer.is_null() {
        "invalid Rust browser module".into()
    } else {
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    })
}
fn module() -> io::Result<Rc<Module>> {
    let file = File::open(module_path()?)?;
    let info = file.metadata()?;
    let identity = (info.dev(), info.ino());
    MODULES.with(|modules| {
        if let Some(current) = modules
            .borrow()
            .last()
            .filter(|module| module.identity == identity)
        {
            return Ok(current.clone());
        }
        // A distinct still-open descriptor avoids dlopen's pathname cache after atomic installation.
        let path = CString::new(format!("/proc/self/fd/{}", file.as_raw_fd())).unwrap();
        let handle = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(loader_error());
        }
        let library = UnpublishedLibrary(handle);
        let symbol = unsafe { libc::dlsym(handle, c"worminal_rust_browser_module".as_ptr()) };
        if symbol.is_null() {
            return Err(loader_error());
        }
        let entry: unsafe extern "C" fn(u32) -> *const WorminalRustBrowser =
            unsafe { std::mem::transmute(symbol) };
        let api = unsafe { entry(WORMINAL_RUST_BROWSER_ABI).as_ref() }
            .ok_or_else(|| io::Error::other("incompatible Rust browser module"))?;
        if api.profile.is_none()
            || api.create.is_none()
            || api.widget.is_none()
            || api.focus.is_none()
            || api.navigate.is_none()
            || api.shortcut.is_none()
            || api.zoom.is_none()
            || api.background.is_none()
            || api.destroy.is_none()
            || api.retire.is_none()
        {
            return Err(io::Error::other("incomplete Rust browser module"));
        }
        let module = Rc::new(Module {
            api,
            identity,
            _file: file,
        });
        // Only validated modules stay resident. Rejected modules must not poison a reused /proc/fd path.
        std::mem::forget(library);
        let old = modules.borrow().last().cloned();
        modules.borrow_mut().push(module.clone());
        unsafe {
            module.api.retire.unwrap()(0);
        }
        if let Some(old) = old {
            unsafe {
                old.api.retire.unwrap()(1);
            }
        }
        Ok(module)
    })
}
fn profile(module: &Module) -> io::Result<glib::Object> {
    PROFILE.with(|slot| {
        if let Some(profile) = slot.borrow().as_ref() {
            return Ok(profile.object.clone());
        }
        // Launch policy fixes the profile once per owner; forwarded callers cannot change it.
        let name = PROFILE_NAME.with(|slot| *slot.borrow());
        let data = glib::user_data_dir().join(name);
        let cache = glib::user_cache_dir().join(name);
        for directory in [&data, &cache] {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(
                data.join(
                    WORMINAL_BROWSER_PROFILE_LOCK,
                ),
            )?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::other(
                "Rust browser profile is already open in another owner",
            ));
        }
        let data = CString::new(data.as_os_str().as_bytes())?;
        let cache = CString::new(cache.as_os_str().as_bytes())?;
        let pointer = unsafe { module.api.profile.unwrap()(data.as_ptr(), cache.as_ptr()) };
        if pointer.is_null() {
            return Err(io::Error::other(
                "could not initialize Rust browser profile",
            ));
        }
        let object: glib::Object = unsafe { from_glib_full(pointer.cast()) };
        *slot.borrow_mut() = Some(Profile {
            object: object.clone(),
            _lock: lock,
        });
        Ok(object)
    })
}
unsafe extern "C" fn event(context: *mut c_void, kind: u32, value: *const libc::c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let callback = unsafe { &*context.cast::<Callback>() }.clone();
        let value = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        // Leave the module before application operations can destroy/reparent this very view.
        glib::idle_add_local_once(move || callback(kind, &value));
    }));
}
impl Browser {
    pub fn new(url: &str, callback: impl Fn(u32, &str) + 'static) -> io::Result<Self> {
        let module = module()?;
        let profile = profile(&module)?;
        let mut callback: Box<Callback> = Box::new(Rc::new(callback));
        let url = CString::new(url)?;
        let handle = unsafe {
            module.api.create.unwrap()(
                profile.as_ptr().cast(),
                url.as_ptr(),
                Some(event),
                (&mut *callback as *mut Callback).cast(),
            )
        };
        if handle.is_null() {
            return Err(io::Error::other("could not create browser view"));
        }
        let pointer = unsafe { module.api.widget.unwrap()(handle) };
        if pointer.is_null() {
            unsafe {
                module.api.destroy.unwrap()(handle);
            }
            return Err(io::Error::other("browser returned no widget"));
        }
        let widget = unsafe { from_glib_none(pointer.cast::<gtk::ffi::GtkWidget>()) };
        Ok(Self(Rc::new(Inner {
            module,
            handle,
            widget,
            _callback: callback,
        })))
    }
    pub fn widget(&self) -> &gtk::Widget {
        &self.0.widget
    }
    pub fn focus(&self) {
        unsafe {
            self.0.module.api.focus.unwrap()(self.0.handle);
        }
    }
    pub fn navigate(&self, url: &str) {
        if let Ok(url) = CString::new(url) {
            unsafe {
                self.0.module.api.navigate.unwrap()(self.0.handle, url.as_ptr());
            }
        }
    }
    pub fn shortcut(&self, event: &gtk::gdk::EventKey) -> bool {
        unsafe {
            self.0.module.api.shortcut.unwrap()(
                self.0.handle,
                *event.keyval(),
                event.state().bits(),
            ) != 0
        }
    }
    pub fn zoom(&self, zoom: f64) {
        unsafe {
            self.0.module.api.zoom.unwrap()(self.0.handle, zoom);
        }
    }
    pub fn background(&self, color: gtk::gdk::RGBA) {
        unsafe {
            self.0.module.api.background.unwrap()(
                self.0.handle,
                color.red(),
                color.green(),
                color.blue(),
                color.alpha(),
            );
        }
    }
}
