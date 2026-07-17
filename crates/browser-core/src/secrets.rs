use std::fmt;

pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError>;
    fn get(&self, key: &str) -> Result<String, SecretError>;
    fn delete(&self, key: &str) -> Result<(), SecretError>;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SecretError(pub String);

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SecretError {}

#[derive(Default)]
pub struct NativeSecretStore;

fn validate_key(key: &str) -> Result<(), SecretError> {
    if key.is_empty() || key.len() > 240 || !key.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte)) {
        return Err(SecretError("invalid secret key".into()));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
impl SecretStore for NativeSecretStore {
    fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        validate_key(key)?;
        let mut child = Command::new("secret-tool")
            .args(["store", "--label=uBar browser credential", "application", "ubar", "key", key])
            .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().map_err(|error| SecretError(format!("OS secret service unavailable: {error}")))?;
        child.stdin.take().ok_or_else(|| SecretError("secret-tool stdin unavailable".into()))?
            .write_all(secret.as_bytes()).map_err(|error| SecretError(error.to_string()))?;
        if child.wait().map_err(|error| SecretError(error.to_string()))?.success() { Ok(()) }
        else { Err(SecretError("OS secret service rejected credential".into())) }
    }

    fn get(&self, key: &str) -> Result<String, SecretError> {
        use std::process::Command;
        validate_key(key)?;
        let output = Command::new("secret-tool")
            .args(["lookup", "application", "ubar", "key", key])
            .output().map_err(|error| SecretError(format!("OS secret service unavailable: {error}")))?;
        if !output.status.success() { return Err(SecretError("credential not found".into())); }
        String::from_utf8(output.stdout).map(|value| value.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(|_| SecretError("OS secret service returned invalid UTF-8".into()))
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        use std::process::Command;
        validate_key(key)?;
        let status = Command::new("secret-tool")
            .args(["clear", "application", "ubar", "key", key])
            .status().map_err(|error| SecretError(format!("OS secret service unavailable: {error}")))?;
        if status.success() { Ok(()) } else { Err(SecretError("credential not found".into())) }
    }
}

#[cfg(target_os = "macos")]
mod apple {
    use super::{NativeSecretStore, SecretError, SecretStore, validate_key};
    use core::ffi::{c_char, c_void};
    use std::ptr;

    type Status = i32;
    type ItemRef = *mut c_void;
    const OK: Status = 0;
    const NOT_FOUND: Status = -25300;
    const SERVICE: &[u8] = b"org.ubar.browser";

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainFindGenericPassword(keychain: *const c_void, service_len: u32, service: *const c_char,
            account_len: u32, account: *const c_char, password_len: *mut u32,
            password: *mut *mut c_void, item: *mut ItemRef) -> Status;
        fn SecKeychainAddGenericPassword(keychain: *const c_void, service_len: u32, service: *const c_char,
            account_len: u32, account: *const c_char, password_len: u32,
            password: *const c_void, item: *mut ItemRef) -> Status;
        fn SecKeychainItemModifyAttributesAndData(item: ItemRef, attributes: *const c_void,
            password_len: u32, password: *const c_void) -> Status;
        fn SecKeychainItemDelete(item: ItemRef) -> Status;
        fn SecKeychainItemFreeContent(attributes: *const c_void, data: *mut c_void) -> Status;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { fn CFRelease(value: *const c_void); }

    fn find(key: &str) -> (Status, ItemRef, *mut c_void, u32) {
        let (mut item, mut data, mut len) = (ptr::null_mut(), ptr::null_mut(), 0);
        let status = unsafe { SecKeychainFindGenericPassword(ptr::null(), SERVICE.len() as u32,
            SERVICE.as_ptr().cast(), key.len() as u32, key.as_ptr().cast(), &mut len, &mut data, &mut item) };
        (status, item, data, len)
    }

    impl SecretStore for NativeSecretStore {
        fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
            validate_key(key)?;
            let (status, item, data, _) = find(key);
            if !data.is_null() { unsafe { SecKeychainItemFreeContent(ptr::null(), data); } }
            let status = if status == OK {
                let result = unsafe { SecKeychainItemModifyAttributesAndData(item, ptr::null(), secret.len() as u32, secret.as_ptr().cast()) };
                unsafe { CFRelease(item); }
                result
            } else if status == NOT_FOUND {
                unsafe { SecKeychainAddGenericPassword(ptr::null(), SERVICE.len() as u32, SERVICE.as_ptr().cast(),
                    key.len() as u32, key.as_ptr().cast(), secret.len() as u32, secret.as_ptr().cast(), ptr::null_mut()) }
            } else { status };
            if status == OK { Ok(()) } else { Err(SecretError(format!("Keychain error {status}"))) }
        }

        fn get(&self, key: &str) -> Result<String, SecretError> {
            validate_key(key)?;
            let (status, item, data, len) = find(key);
            if status != OK { return Err(SecretError("credential not found".into())); }
            let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) };
            let result = String::from_utf8(bytes.to_vec()).map_err(|_| SecretError("Keychain value is not UTF-8".into()));
            unsafe { SecKeychainItemFreeContent(ptr::null(), data); CFRelease(item); }
            result
        }

        fn delete(&self, key: &str) -> Result<(), SecretError> {
            validate_key(key)?;
            let (status, item, data, _) = find(key);
            if !data.is_null() { unsafe { SecKeychainItemFreeContent(ptr::null(), data); } }
            if status != OK { return Err(SecretError("credential not found".into())); }
            let result = unsafe { SecKeychainItemDelete(item) };
            unsafe { CFRelease(item); }
            if result == OK { Ok(()) } else { Err(SecretError(format!("Keychain error {result}"))) }
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::{NativeSecretStore, SecretError, SecretStore, validate_key};
    use core::ffi::c_void;
    use std::ptr;

    #[repr(C)] struct FileTime { low: u32, high: u32 }
    #[repr(C)] struct CredentialW {
        flags: u32, kind: u32, target_name: *mut u16, comment: *mut u16, last_written: FileTime,
        blob_size: u32, blob: *mut u8, persist: u32, attribute_count: u32,
        attributes: *mut c_void, target_alias: *mut u16, user_name: *mut u16,
    }
    const GENERIC: u32 = 1;
    const LOCAL_MACHINE: u32 = 2;
    #[link(name = "Advapi32")]
    unsafe extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
        fn CredReadW(target: *const u16, kind: u32, flags: u32, credential: *mut *mut CredentialW) -> i32;
        fn CredDeleteW(target: *const u16, kind: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut c_void);
    }
    fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }
    fn target(key: &str) -> Vec<u16> { wide(&format!("uBar:{key}")) }

    impl SecretStore for NativeSecretStore {
        fn set(&self, key: &str, secret: &str) -> Result<(), SecretError> {
            validate_key(key)?;
            let mut target = target(key);
            let mut user = wide("uBar");
            let mut blob: Vec<u8> = secret.encode_utf16().flat_map(u16::to_le_bytes).collect();
            if blob.len() > 2560 { return Err(SecretError("credential exceeds Windows vault limit".into())); }
            let credential = CredentialW { flags: 0, kind: GENERIC, target_name: target.as_mut_ptr(), comment: ptr::null_mut(),
                last_written: FileTime { low: 0, high: 0 }, blob_size: blob.len() as u32, blob: blob.as_mut_ptr(),
                persist: LOCAL_MACHINE, attribute_count: 0, attributes: ptr::null_mut(), target_alias: ptr::null_mut(), user_name: user.as_mut_ptr() };
            if unsafe { CredWriteW(&credential, 0) } != 0 { Ok(()) } else { Err(SecretError("Windows Credential Manager rejected credential".into())) }
        }
        fn get(&self, key: &str) -> Result<String, SecretError> {
            validate_key(key)?;
            let target = target(key);
            let mut credential = ptr::null_mut();
            if unsafe { CredReadW(target.as_ptr(), GENERIC, 0, &mut credential) } == 0 { return Err(SecretError("credential not found".into())); }
            let value = unsafe {
                let record = &*credential;
                let bytes = std::slice::from_raw_parts(record.blob, record.blob_size as usize);
                let words: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
                String::from_utf16(&words)
            };
            unsafe { CredFree(credential.cast()); }
            value.map_err(|_| SecretError("Windows vault value is not UTF-16".into()))
        }
        fn delete(&self, key: &str) -> Result<(), SecretError> {
            validate_key(key)?;
            if unsafe { CredDeleteW(target(key).as_ptr(), GENERIC, 0) } != 0 { Ok(()) } else { Err(SecretError("credential not found".into())) }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl SecretStore for NativeSecretStore {
    fn set(&self, _: &str, _: &str) -> Result<(), SecretError> { Err(SecretError("OS secret store unsupported".into())) }
    fn get(&self, _: &str) -> Result<String, SecretError> { Err(SecretError("OS secret store unsupported".into())) }
    fn delete(&self, _: &str) -> Result<(), SecretError> { Err(SecretError("OS secret store unsupported".into())) }
}
