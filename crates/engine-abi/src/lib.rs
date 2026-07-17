#![deny(unsafe_op_in_unsafe_fn)]

use core::ffi::{c_char, c_void};

pub const UBAR_ENGINE_ABI_V1: u32 = 1;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UbarResult {
    Ok = 0,
    InvalidArgument = 1,
    Unsupported = 2,
    PermissionDenied = 3,
    EngineFailure = 4,
    AbiMismatch = 5,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UbarProfileKind {
    Normal = 0,
    Private = 1,
}

#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UbarEventKind {
    NavigationStarted = 0,
    NavigationCommitted = 1,
    NavigationFinished = 2,
    TitleChanged = 3,
    UriChanged = 4,
    RendererCrashed = 5,
    PermissionRequested = 6,
    DownloadRequested = 7,
    MemoryChanged = 8,
    ExtensionMessage = 9,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UbarBytes {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct UbarOwnedBytes {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UbarProfileConfigV1 {
    pub struct_size: u32,
    pub kind: UbarProfileKind,
    pub data_directory_utf8: UbarBytes,
    pub cache_directory_utf8: UbarBytes,
    pub memory_target_bytes: u64,
    pub memory_ceiling_bytes: u64,
    pub partition_third_party_storage: bool,
    pub block_third_party_cookies: bool,
    pub require_sandbox: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UbarViewConfigV1 {
    pub struct_size: u32,
    pub native_parent: *mut c_void,
    pub width: u32,
    pub height: u32,
    pub device_scale: f64,
    pub initially_visible: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UbarEventV1 {
    pub struct_size: u32,
    pub kind: UbarEventKind,
    pub view: u64,
    pub text_utf8: UbarBytes,
    pub value: u64,
}

pub type UbarEventCallbackV1 =
    unsafe extern "C" fn(user_data: *mut c_void, event: *const UbarEventV1);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UbarCallbacksV1 {
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub event: Option<UbarEventCallbackV1>,
}

pub type CreateProfileFn = unsafe extern "C" fn(
    config: *const UbarProfileConfigV1,
    profile_out: *mut u64,
) -> UbarResult;
pub type DestroyProfileFn = unsafe extern "C" fn(profile: u64) -> UbarResult;
pub type CreateViewFn = unsafe extern "C" fn(
    profile: u64,
    config: *const UbarViewConfigV1,
    callbacks: *const UbarCallbacksV1,
    view_out: *mut u64,
) -> UbarResult;
pub type DestroyViewFn = unsafe extern "C" fn(view: u64) -> UbarResult;
pub type ViewBytesFn = unsafe extern "C" fn(view: u64, value: UbarBytes) -> UbarResult;
pub type ViewBoolFn = unsafe extern "C" fn(view: u64, value: bool) -> UbarResult;
pub type ViewF64Fn = unsafe extern "C" fn(view: u64, value: f64) -> UbarResult;
pub type ViewFn = unsafe extern "C" fn(view: u64) -> UbarResult;
pub type ProfileBytesFn = unsafe extern "C" fn(profile: u64, value: UbarBytes) -> UbarResult;
pub type ProfileJsonFn = unsafe extern "C" fn(
    profile: u64,
    request_json_utf8: UbarBytes,
    response_json_utf8_out: *mut UbarOwnedBytes,
) -> UbarResult;
pub type RegisterCdmFn = unsafe extern "C" fn(
    profile: u64,
    key_system_utf8: UbarBytes,
    library_path_utf8: UbarBytes,
    manifest_json_utf8: UbarBytes,
) -> UbarResult;
pub type FreeBytesFn = unsafe extern "C" fn(value: UbarOwnedBytes);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct UbarEngineApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub engine_name: *const c_char,
    pub create_profile: CreateProfileFn,
    pub destroy_profile: DestroyProfileFn,
    pub create_view: CreateViewFn,
    pub destroy_view: DestroyViewFn,
    pub navigate: ViewBytesFn,
    pub set_visible: ViewBoolFn,
    pub set_zoom: ViewF64Fn,
    pub suspend: ViewFn,
    pub resume: ViewFn,
    pub set_request_policy_json: ProfileBytesFn,
    pub register_cdm: RegisterCdmFn,
    pub free_bytes: FreeBytesFn,
    pub go_back: ViewFn,
    pub go_forward: ViewFn,
    pub reload: ViewFn,
    pub stop: ViewFn,
    pub extension_control_json: ProfileJsonFn,
    pub browser_control_json: ProfileJsonFn,
}

// The table is immutable after construction. Its raw pointer targets a static,
// NUL-terminated engine name; every other field is a function pointer.
unsafe impl Sync for UbarEngineApiV1 {}

pub type GetEngineApiFn =
    unsafe extern "C" fn(requested_abi: u32, api_out: *mut *const UbarEngineApiV1) -> UbarResult;

impl UbarProfileConfigV1 {
    pub fn normal() -> Self {
        Self {
            struct_size: core::mem::size_of::<Self>() as u32,
            kind: UbarProfileKind::Normal,
            data_directory_utf8: UbarBytes::default(),
            cache_directory_utf8: UbarBytes::default(),
            memory_target_bytes: 1024 * 1024 * 1024,
            memory_ceiling_bytes: 1280 * 1024 * 1024,
            partition_third_party_storage: true,
            block_third_party_cookies: true,
            require_sandbox: true,
        }
    }

    pub fn private() -> Self {
        Self {
            kind: UbarProfileKind::Private,
            data_directory_utf8: UbarBytes::default(),
            cache_directory_utf8: UbarBytes::default(),
            ..Self::normal()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_profile_has_no_persistent_paths() {
        let profile = UbarProfileConfigV1::private();
        assert_eq!(profile.kind, UbarProfileKind::Private);
        assert_eq!(profile.data_directory_utf8.len, 0);
        assert_eq!(profile.cache_directory_utf8.len, 0);
        assert!(profile.require_sandbox);
    }

    #[test]
    fn abi_struct_identifies_itself() {
        assert_eq!(UBAR_ENGINE_ABI_V1, 1);
        assert!(core::mem::size_of::<UbarEngineApiV1>() > 64);
    }
}
