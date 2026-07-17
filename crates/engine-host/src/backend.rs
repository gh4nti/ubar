use libloading::{Library, Symbol};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use ubar_engine_abi::{
    UbarBytes, UbarCallbacksV1, UbarProfileConfigV1, UbarResult, UbarViewConfigV1,
};

pub const UBAR_WEBKIT_PORT_ABI_V1: u32 = 1;

type CreateProfileFn = unsafe extern "C" fn(u64, *const UbarProfileConfigV1) -> UbarResult;
type DestroyProfileFn = unsafe extern "C" fn(u64) -> UbarResult;
type CreateViewFn = unsafe extern "C" fn(
    u64,
    u64,
    *const UbarViewConfigV1,
    *const UbarCallbacksV1,
) -> UbarResult;
type DestroyViewFn = unsafe extern "C" fn(u64) -> UbarResult;
type ViewBytesFn = unsafe extern "C" fn(u64, UbarBytes) -> UbarResult;
type ViewTwoBytesFn = unsafe extern "C" fn(u64, UbarBytes, UbarBytes) -> UbarResult;
type ViewBoolFn = unsafe extern "C" fn(u64, bool) -> UbarResult;
type ViewF64Fn = unsafe extern "C" fn(u64, f64) -> UbarResult;
type ViewFn = unsafe extern "C" fn(u64) -> UbarResult;
type ProfileBytesFn = unsafe extern "C" fn(u64, UbarBytes) -> UbarResult;

#[repr(C)]
#[derive(Clone, Copy)]
struct WebKitPortApiV1 {
    abi_version: u32,
    struct_size: u32,
    create_profile: CreateProfileFn,
    destroy_profile: DestroyProfileFn,
    create_view: CreateViewFn,
    destroy_view: DestroyViewFn,
    navigate: ViewBytesFn,
    set_visible: ViewBoolFn,
    set_zoom: ViewF64Fn,
    suspend: ViewFn,
    resume: ViewFn,
    go_back: ViewFn,
    go_forward: ViewFn,
    reload: ViewFn,
    stop: ViewFn,
    set_request_policy_json: ProfileBytesFn,
    evaluate_extension_script: ViewTwoBytesFn,
    create_headless_view: unsafe extern "C" fn(u64, u64, *const UbarCallbacksV1) -> UbarResult,
}

type GetPortApiFn = unsafe extern "C" fn(u32, *mut *const WebKitPortApiV1) -> UbarResult;

pub struct WebKitBackend {
    _library: Library,
    api: WebKitPortApiV1,
}

impl WebKitBackend {
    pub fn load_default() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let directory = executable.parent().ok_or("executable has no parent directory")?;
        let filename: &OsStr = if cfg!(target_os = "windows") {
            OsStr::new("ubar_webkit_port.dll")
        } else if cfg!(target_os = "macos") {
            OsStr::new("libubar_webkit_port.dylib")
        } else {
            OsStr::new("libubar_webkit_port.so")
        };
        Self::load(directory.join(filename))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path: PathBuf = path.as_ref().to_path_buf();
        if !path.is_absolute() || !path.is_file() {
            return Err(format!("WebKit port library is missing: {}", path.display()));
        }
        let library = unsafe { Library::new(&path) }.map_err(|error| error.to_string())?;
        let get_api: Symbol<GetPortApiFn> = unsafe { library.get(b"ubar_webkit_port_get_api\0") }
            .map_err(|error| error.to_string())?;
        let mut api = std::ptr::null();
        let result = unsafe { get_api(UBAR_WEBKIT_PORT_ABI_V1, &mut api) };
        if result != UbarResult::Ok || api.is_null() {
            return Err("WebKit port rejected ABI v1".into());
        }
        let api = unsafe { *api };
        if api.abi_version != UBAR_WEBKIT_PORT_ABI_V1
            || api.struct_size < std::mem::size_of::<WebKitPortApiV1>() as u32
        {
            return Err("WebKit port returned an incompatible API table".into());
        }
        Ok(Self { _library: library, api })
    }

    pub fn create_profile(&self, id: u64, config: &UbarProfileConfigV1) -> UbarResult {
        unsafe { (self.api.create_profile)(id, config) }
    }
    pub fn destroy_profile(&self, id: u64) -> UbarResult {
        unsafe { (self.api.destroy_profile)(id) }
    }
    pub fn create_view(
        &self,
        id: u64,
        profile: u64,
        config: &UbarViewConfigV1,
        callbacks: *const UbarCallbacksV1,
    ) -> UbarResult {
        unsafe { (self.api.create_view)(id, profile, config, callbacks) }
    }
    pub fn destroy_view(&self, id: u64) -> UbarResult {
        unsafe { (self.api.destroy_view)(id) }
    }
    pub fn navigate(&self, id: u64, value: UbarBytes) -> UbarResult {
        unsafe { (self.api.navigate)(id, value) }
    }
    pub fn set_visible(&self, id: u64, value: bool) -> UbarResult {
        unsafe { (self.api.set_visible)(id, value) }
    }
    pub fn set_zoom(&self, id: u64, value: f64) -> UbarResult {
        unsafe { (self.api.set_zoom)(id, value) }
    }
    pub fn suspend(&self, id: u64) -> UbarResult { unsafe { (self.api.suspend)(id) } }
    pub fn resume(&self, id: u64) -> UbarResult { unsafe { (self.api.resume)(id) } }
    pub fn go_back(&self, id: u64) -> UbarResult { unsafe { (self.api.go_back)(id) } }
    pub fn go_forward(&self, id: u64) -> UbarResult { unsafe { (self.api.go_forward)(id) } }
    pub fn reload(&self, id: u64) -> UbarResult { unsafe { (self.api.reload)(id) } }
    pub fn stop(&self, id: u64) -> UbarResult { unsafe { (self.api.stop)(id) } }
    pub fn set_request_policy_json(&self, profile: u64, value: UbarBytes) -> UbarResult {
        unsafe { (self.api.set_request_policy_json)(profile, value) }
    }

    pub fn evaluate_extension_script(
        &self,
        view: u64,
        world: UbarBytes,
        script: UbarBytes,
    ) -> UbarResult {
        unsafe { (self.api.evaluate_extension_script)(view, world, script) }
    }

    pub fn create_headless_view(
        &self,
        id: u64,
        profile: u64,
        callbacks: *const UbarCallbacksV1,
    ) -> UbarResult {
        unsafe { (self.api.create_headless_view)(id, profile, callbacks) }
    }
}
