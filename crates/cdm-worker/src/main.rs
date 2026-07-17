#![deny(unsafe_op_in_unsafe_fn)]

use libloading::{Library, Symbol};
use std::env;
use std::io::{stdin, stdout};
use std::path::{Path, PathBuf};
use ubar_drm_host::adapter::*;
use ubar_drm_host::broker::FramedJson;
use ubar_drm_host::widevine::{CdmRequest, CdmResponse};

struct Adapter {
    _library: Library,
    api: CdmAdapterApiV1,
}

impl Adapter {
    fn load(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() || !path.is_file() {
            return Err("licensed CDM adapter path is invalid".into());
        }
        let library = unsafe { Library::new(path) }.map_err(|error| error.to_string())?;
        let get_api: Symbol<GetCdmAdapterApiFn> = unsafe {
            library.get(b"ubar_get_cdm_adapter_api\0")
        }.map_err(|error| error.to_string())?;
        let mut api = std::ptr::null();
        let result = unsafe { get_api(UBAR_CDM_ADAPTER_ABI_V1, &mut api) };
        if result != AdapterResult::Ok || api.is_null() {
            return Err("licensed CDM adapter rejected ABI v1".into());
        }
        let api = unsafe { *api };
        if api.abi_version != UBAR_CDM_ADAPTER_ABI_V1
            || api.struct_size < std::mem::size_of::<CdmAdapterApiV1>() as u32
        {
            return Err("licensed CDM adapter returned an incompatible API".into());
        }
        Ok(Self { _library: library, api })
    }

    fn take_owned(&self, value: AdapterOwnedBytes) -> Result<Vec<u8>, String> {
        if value.len == 0 {
            unsafe { (self.api.free_bytes)(self.api.user_data, value) };
            return Ok(Vec::new());
        }
        if value.data.is_null() || value.capacity < value.len {
            return Err("licensed CDM adapter returned invalid memory".into());
        }
        let bytes = unsafe { std::slice::from_raw_parts(value.data, value.len) }.to_vec();
        unsafe { (self.api.free_bytes)(self.api.user_data, value) };
        Ok(bytes)
    }

    fn initialize(&self, component: &Path, host_version: u32) -> Result<String, String> {
        let component = component.to_str().ok_or("component path is not UTF-8")?.as_bytes();
        let mut output = AdapterOwnedBytes::default();
        let result = unsafe { (self.api.initialize)(
            self.api.user_data,
            AdapterBytes { data: component.as_ptr(), len: component.len() },
            host_version,
            &mut output,
        ) };
        if result != AdapterResult::Ok { return Err(format!("CDM initialize failed: {result:?}")); }
        String::from_utf8(self.take_owned(output)?).map_err(|error| error.to_string())
    }

    fn transact(&self, request: &CdmRequest) -> Result<CdmResponse, String> {
        let request = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        let mut output = AdapterOwnedBytes::default();
        let result = unsafe { (self.api.transact)(
            self.api.user_data,
            AdapterBytes { data: request.as_ptr(), len: request.len() }, &mut output,
        ) };
        if result != AdapterResult::Ok { return Err(format!("CDM transaction failed: {result:?}")); }
        serde_json::from_slice(&self.take_owned(output)?).map_err(|error| error.to_string())
    }
}

impl Drop for Adapter {
    fn drop(&mut self) { unsafe { (self.api.shutdown)(self.api.user_data) } }
}

fn main() {
    if let Err(error) = run() {
        let mut channel = FramedJson::new(stdin().lock(), stdout().lock());
        let _ = channel.send(&CdmResponse::Error { request_id: None, reason: error });
        std::process::exit(2);
    }
}

#[cfg(unix)]
fn apply_memory_limit() -> Result<(), String> {
    let Some(value) = env::var_os("UBAR_CDM_MEMORY_LIMIT_BYTES") else { return Ok(()) };
    let bytes = value.to_string_lossy().parse::<libc::rlim_t>()
        .map_err(|_| "invalid CDM memory limit")?;
    if bytes == 0 { return Err("CDM memory limit must be positive".into()); }
    let limit = libc::rlimit { rlim_cur: bytes, rlim_max: bytes };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &limit) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_memory_limit() -> Result<(), String> { Ok(()) }

fn run() -> Result<(), String> {
    apply_memory_limit()?;
    let adapter_path = env::args_os().nth(1).map(PathBuf::from)
        .ok_or("worker requires licensed adapter path")?;
    let adapter = Adapter::load(&adapter_path)?;
    let mut channel = FramedJson::new(stdin().lock(), stdout().lock());
    let initialize: CdmRequest = channel.receive()?;
    let (key_system, component_path, host_version) = match initialize {
        CdmRequest::Initialize { key_system, component_path, host_version } => {
            (key_system, component_path, host_version)
        }
        _ => return Err("first CDM request must initialize".into()),
    };
    if key_system != "com.widevine.alpha" { return Err("unsupported key system".into()); }
    let version = adapter.initialize(&component_path, host_version)?;
    channel.send(&CdmResponse::Initialized { module_version: version })?;
    loop {
        let request: CdmRequest = channel.receive()?;
        if matches!(request, CdmRequest::Shutdown) { return Ok(()); }
        let request_id = match &request {
            CdmRequest::Decrypt { request_id, .. } => Some(*request_id),
            _ => None,
        };
        let response = adapter.transact(&request).unwrap_or_else(|reason| CdmResponse::Error {
            request_id,
            reason,
        });
        channel.send(&response)?;
    }
}
