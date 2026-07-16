#![deny(unsafe_op_in_unsafe_fn)]

mod backend;

use backend::WebKitBackend;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use ubar_browser_core::BrowserCore;
use ubar_drm_host::widevine::{
    AuthorizedWidevine, CdmRegistrationBundle, Ed25519AuthorizationVerifier,
};
use ubar_engine_abi::*;

const DEFAULT_REQUEST_POLICY: &[u8] =
    include_bytes!("../../../config/default-content-blocker.json");

struct HostState {
    core: BrowserCore,
    zoom: BTreeMap<u64, f64>,
    cdms: BTreeMap<u64, AuthorizedWidevine>,
    request_policies: BTreeMap<u64, serde_json::Value>,
    backend: Option<WebKitBackend>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            core: BrowserCore::default(),
            zoom: BTreeMap::new(),
            cdms: BTreeMap::new(),
            request_policies: BTreeMap::new(),
            backend: WebKitBackend::load_default().ok(),
        }
    }
}

fn host() -> &'static Mutex<HostState> {
    static HOST: OnceLock<Mutex<HostState>> = OnceLock::new();
    HOST.get_or_init(|| Mutex::new(HostState::default()))
}

unsafe fn bytes<'a>(value: UbarBytes) -> Result<&'a [u8], UbarResult> {
    if value.len == 0 {
        return Ok(&[]);
    }
    if value.data.is_null() {
        return Err(UbarResult::InvalidArgument);
    }
    Ok(unsafe { std::slice::from_raw_parts(value.data, value.len) })
}

unsafe fn text<'a>(value: UbarBytes) -> Result<&'a str, UbarResult> {
    std::str::from_utf8(unsafe { bytes(value)? }).map_err(|_| UbarResult::InvalidArgument)
}

unsafe extern "C" fn create_profile(
    config: *const UbarProfileConfigV1,
    profile_out: *mut u64,
) -> UbarResult {
    if config.is_null() || profile_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    let config = unsafe { &*config };
    if config.struct_size < std::mem::size_of::<UbarProfileConfigV1>() as u32 {
        return UbarResult::AbiMismatch;
    }
    if !config.require_sandbox
        || !config.partition_third_party_storage
        || !config.block_third_party_cookies
    {
        return UbarResult::PermissionDenied;
    }
    if config.kind == UbarProfileKind::Private
        && (config.data_directory_utf8.len != 0 || config.cache_directory_utf8.len != 0)
    {
        return UbarResult::PermissionDenied;
    }
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if host.backend.is_none() {
        return UbarResult::EngineFailure;
    }
    let profile = host.core.create_profile(config.kind);
    let result = host.backend.as_ref().unwrap().create_profile(profile.id, config);
    if result != UbarResult::Ok {
        host.core.destroy_profile(profile.id);
        return result;
    }
    let default_policy = UbarBytes {
        data: DEFAULT_REQUEST_POLICY.as_ptr(),
        len: DEFAULT_REQUEST_POLICY.len(),
    };
    let result = host.backend.as_ref().unwrap().set_request_policy_json(profile.id, default_policy);
    if result != UbarResult::Ok {
        host.backend.as_ref().unwrap().destroy_profile(profile.id);
        host.core.destroy_profile(profile.id);
        return result;
    }
    if let Ok(policy) = serde_json::from_slice(DEFAULT_REQUEST_POLICY) {
        host.request_policies.insert(profile.id, policy);
    }
    unsafe { *profile_out = profile.id };
    UbarResult::Ok
}

unsafe extern "C" fn destroy_profile(profile: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) {
        return UbarResult::InvalidArgument;
    }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.destroy_profile(profile);
    if result != UbarResult::Ok {
        return result;
    }
    host.cdms.remove(&profile);
    host.request_policies.remove(&profile);
    if host.core.destroy_profile(profile) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

unsafe extern "C" fn create_view(
    profile: u64,
    config: *const UbarViewConfigV1,
    callbacks: *const UbarCallbacksV1,
    view_out: *mut u64,
) -> UbarResult {
    if config.is_null() || view_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    let config = unsafe { &*config };
    if config.struct_size < std::mem::size_of::<UbarViewConfigV1>() as u32 {
        return UbarResult::AbiMismatch;
    }
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if host.backend.is_none() {
        return UbarResult::EngineFailure;
    }
    let Some(view) = host.core.create_view(profile, "about:blank") else {
        return UbarResult::InvalidArgument;
    };
    host.core.set_visible(view.id, config.initially_visible);
    if !callbacks.is_null()
        && unsafe { (*callbacks).struct_size } < std::mem::size_of::<UbarCallbacksV1>() as u32
    {
        host.core.destroy_view(view.id);
        return UbarResult::AbiMismatch;
    }
    let result = host.backend.as_ref().unwrap().create_view(view.id, profile, config, callbacks);
    if result != UbarResult::Ok {
        host.core.destroy_view(view.id);
        return result;
    }
    host.zoom.insert(view.id, 1.0);
    unsafe { *view_out = view.id };
    UbarResult::Ok
}

unsafe extern "C" fn destroy_view(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.destroy_view(view);
    if result != UbarResult::Ok {
        return result;
    }
    host.zoom.remove(&view);
    if host.core.destroy_view(view) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

unsafe extern "C" fn navigate(view: u64, value: UbarBytes) -> UbarResult {
    let uri = match unsafe { text(value) } {
        Ok(uri) if uri.starts_with("http://") || uri.starts_with("https://") || uri == "about:blank" => uri,
        _ => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.navigate(view, value);
    if result != UbarResult::Ok {
        return result;
    }
    if !host.core.navigate(view, uri) {
        return UbarResult::InvalidArgument;
    }
    UbarResult::Ok
}

unsafe extern "C" fn set_visible(view: u64, visible: bool) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_visible(view, visible);
    if result != UbarResult::Ok { return result; }
    if host.core.set_visible(view, visible) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

unsafe extern "C" fn set_zoom(view: u64, zoom: f64) -> UbarResult {
    if !(0.5..=5.0).contains(&zoom) {
        return UbarResult::InvalidArgument;
    }
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.zoom.contains_key(&view) {
        return UbarResult::InvalidArgument;
    }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_zoom(view, zoom);
    if result != UbarResult::Ok { return result; }
    host.zoom.insert(view, zoom);
    UbarResult::Ok
}

unsafe extern "C" fn suspend(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.suspend(view);
    if result != UbarResult::Ok { return result; }
    if host.core.suspend(view) {
        UbarResult::Ok
    } else {
        UbarResult::PermissionDenied
    }
}

unsafe extern "C" fn resume(view: u64) -> UbarResult {
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.resume(view);
    if result != UbarResult::Ok { return result; }
    if host.core.resume(view) {
        UbarResult::Ok
    } else {
        UbarResult::InvalidArgument
    }
}

fn backend_view_command(view: u64, command: impl FnOnce(&WebKitBackend, u64) -> UbarResult) -> UbarResult {
    let host = host().lock().unwrap_or_else(|error| error.into_inner());
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    command(backend, view)
}

unsafe extern "C" fn go_back(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::go_back)
}

unsafe extern "C" fn go_forward(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::go_forward)
}

unsafe extern "C" fn reload(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::reload)
}

unsafe extern "C" fn stop(view: u64) -> UbarResult {
    backend_view_command(view, WebKitBackend::stop)
}

unsafe extern "C" fn set_request_policy_json(profile: u64, value: UbarBytes) -> UbarResult {
    let text = match unsafe { text(value) } {
        Ok(text) => text,
        Err(error) => return error,
    };
    let policy = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value @ serde_json::Value::Array(_)) => value,
        _ => return UbarResult::InvalidArgument,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) { return UbarResult::InvalidArgument; }
    let Some(backend) = host.backend.as_ref() else { return UbarResult::EngineFailure };
    let result = backend.set_request_policy_json(profile, value);
    if result == UbarResult::Ok { host.request_policies.insert(profile, policy); }
    result
}

unsafe extern "C" fn register_cdm(
    profile: u64,
    key_system: UbarBytes,
    library_path: UbarBytes,
    manifest_json: UbarBytes,
) -> UbarResult {
    let key_system = match unsafe { text(key_system) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let library_path = match unsafe { text(library_path) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    let bundle_json = match unsafe { bytes(manifest_json) } {
        Ok(value) => value,
        Err(error) => return error,
    };
    if profile == 0
        || key_system != "com.widevine.alpha"
        || library_path.is_empty()
    {
        return UbarResult::InvalidArgument;
    }
    let bundle: CdmRegistrationBundle = match serde_json::from_slice(bundle_json) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let Some(public_key) = option_env!("UBAR_WIDEVINE_AUTH_PUBLIC_KEY_BASE64") else {
        return UbarResult::PermissionDenied;
    };
    let verifier = match Ed25519AuthorizationVerifier::from_base64(public_key) {
        Ok(value) => value,
        Err(_) => return UbarResult::EngineFailure,
    };
    let manifest = match serde_json::to_vec(&bundle.manifest) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let authorization = match serde_json::to_vec(&bundle.authorization) {
        Ok(value) => value,
        Err(_) => return UbarResult::InvalidArgument,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64);
    let cdm = match AuthorizedWidevine::open(
        &PathBuf::from(library_path),
        &manifest,
        &authorization,
        now,
        &verifier,
    ) {
        Ok(value) => value,
        Err(_) => return UbarResult::PermissionDenied,
    };
    let mut host = host().lock().unwrap_or_else(|error| error.into_inner());
    if !host.core.has_profile(profile) {
        return UbarResult::InvalidArgument;
    }
    host.cdms.insert(profile, cdm);
    UbarResult::Ok
}

unsafe extern "C" fn free_bytes(value: UbarOwnedBytes) {
    if !value.data.is_null() && value.capacity >= value.len {
        drop(unsafe { Vec::from_raw_parts(value.data, value.len, value.capacity) });
    }
}

static ENGINE_NAME: &CStr = c"uBar shared engine host";

static API: UbarEngineApiV1 = UbarEngineApiV1 {
    abi_version: UBAR_ENGINE_ABI_V1,
    struct_size: std::mem::size_of::<UbarEngineApiV1>() as u32,
    engine_name: ENGINE_NAME.as_ptr(),
    create_profile,
    destroy_profile,
    create_view,
    destroy_view,
    navigate,
    set_visible,
    set_zoom,
    suspend,
    resume,
    set_request_policy_json,
    register_cdm,
    free_bytes,
    go_back,
    go_forward,
    reload,
    stop,
};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ubar_get_engine_api(
    requested_abi: u32,
    api_out: *mut *const UbarEngineApiV1,
) -> UbarResult {
    if api_out.is_null() {
        return UbarResult::InvalidArgument;
    }
    if requested_abi != UBAR_ENGINE_ABI_V1 {
        return UbarResult::AbiMismatch;
    }
    unsafe { *api_out = &API };
    UbarResult::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_v1_and_rejects_unsandboxed_profiles() {
        let mut api = std::ptr::null();
        assert_eq!(unsafe { ubar_get_engine_api(1, &mut api) }, UbarResult::Ok);
        assert!(!api.is_null());
        let mut config = UbarProfileConfigV1::normal();
        config.require_sandbox = false;
        let mut profile = 0;
        assert_eq!(
            unsafe { ((*api).create_profile)(&config, &mut profile) },
            UbarResult::PermissionDenied
        );
    }

    #[test]
    fn private_profile_rejects_persistent_paths() {
        let data = b"profile";
        let mut config = UbarProfileConfigV1::private();
        config.data_directory_utf8 = UbarBytes {
            data: data.as_ptr(),
            len: data.len(),
        };
        let mut profile = 0;
        assert_eq!(
            unsafe { create_profile(&config, &mut profile) },
            UbarResult::PermissionDenied
        );
    }
}
