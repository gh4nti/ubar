use core::ffi::c_void;

pub const UBAR_CDM_ADAPTER_ABI_V1: u32 = 1;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdapterResult {
    Ok = 0,
    InvalidArgument = 1,
    Unsupported = 2,
    ComponentRejected = 3,
    Failure = 4,
    AbiMismatch = 5,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AdapterBytes {
    pub data: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct AdapterOwnedBytes {
    pub data: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

pub type InitializeFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    component_path_utf8: AdapterBytes,
    host_version: u32,
    module_version_utf8_out: *mut AdapterOwnedBytes,
) -> AdapterResult;
pub type TransactFn = unsafe extern "C" fn(
    user_data: *mut c_void,
    request_json: AdapterBytes,
    response_json_out: *mut AdapterOwnedBytes,
) -> AdapterResult;
pub type ShutdownFn = unsafe extern "C" fn(user_data: *mut c_void);
pub type FreeFn = unsafe extern "C" fn(user_data: *mut c_void, value: AdapterOwnedBytes);

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CdmAdapterApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub user_data: *mut c_void,
    pub initialize: InitializeFn,
    pub transact: TransactFn,
    pub shutdown: ShutdownFn,
    pub free_bytes: FreeFn,
}

// The API table is immutable. Adapter implementations own synchronization for
// the pointed-to state and all calls occur on the isolated worker process.
unsafe impl Sync for CdmAdapterApiV1 {}

pub type GetCdmAdapterApiFn = unsafe extern "C" fn(
    requested_abi: u32,
    api_out: *mut *const CdmAdapterApiV1,
) -> AdapterResult;
