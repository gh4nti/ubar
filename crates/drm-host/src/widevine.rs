use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidevineAuthorization {
    pub license_id: String,
    pub distributor: String,
    pub expires_unix: i64,
    pub component_sha256: String,
    pub allowed_os: Vec<String>,
    pub allowed_arch: Vec<String>,
    pub signature_base64: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidevineManifest {
    pub version: String,
    #[serde(rename = "x-cdm-module-versions")]
    pub module_versions: String,
    #[serde(rename = "x-cdm-interface-versions")]
    pub interface_versions: String,
    #[serde(rename = "x-cdm-host-versions")]
    pub host_versions: String,
    #[serde(default, rename = "x-cdm-codecs")]
    pub codecs: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CdmRegistrationBundle {
    pub manifest: WidevineManifest,
    pub authorization: WidevineAuthorization,
}

#[derive(Clone, Debug)]
pub struct AuthorizedWidevine {
    pub library: PathBuf,
    pub manifest: WidevineManifest,
    pub authorization: WidevineAuthorization,
}

pub trait AuthorizationVerifier {
    fn verify(&self, signed_payload: &[u8], signature: &[u8]) -> bool;
}

pub struct Ed25519AuthorizationVerifier(VerifyingKey);

impl Ed25519AuthorizationVerifier {
    pub fn from_base64(value: &str) -> Result<Self, String> {
        let bytes = STANDARD.decode(value).map_err(|error| error.to_string())?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| "Ed25519 public key must be 32 bytes")?;
        VerifyingKey::from_bytes(&bytes).map(Self).map_err(|error| error.to_string())
    }
}

impl AuthorizationVerifier for Ed25519AuthorizationVerifier {
    fn verify(&self, signed_payload: &[u8], signature: &[u8]) -> bool {
        let Ok(signature) = Signature::try_from(signature) else { return false };
        self.0.verify(signed_payload, &signature).is_ok()
    }
}

#[derive(Serialize)]
struct SignedAuthorization<'a> {
    license_id: &'a str,
    distributor: &'a str,
    expires_unix: i64,
    component_sha256: &'a str,
    allowed_os: &'a [String],
    allowed_arch: &'a [String],
}

impl WidevineAuthorization {
    fn signed_payload(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&SignedAuthorization {
            license_id: &self.license_id,
            distributor: &self.distributor,
            expires_unix: self.expires_unix,
            component_sha256: &self.component_sha256,
            allowed_os: &self.allowed_os,
            allowed_arch: &self.allowed_arch,
        })
        .map_err(|error| error.to_string())
    }
}

impl AuthorizedWidevine {
    pub fn open(
        library: &Path,
        manifest_json: &[u8],
        authorization_json: &[u8],
        now_unix: i64,
        verifier: &dyn AuthorizationVerifier,
    ) -> Result<Self, String> {
        if !library.is_file() {
            return Err("Widevine component library is missing".into());
        }
        let manifest: WidevineManifest =
            serde_json::from_slice(manifest_json).map_err(|error| error.to_string())?;
        if manifest.version.trim().is_empty()
            || manifest.module_versions.trim().is_empty()
            || manifest.interface_versions.trim().is_empty()
            || manifest.host_versions.trim().is_empty()
        {
            return Err("Widevine manifest is incomplete".into());
        }
        let authorization: WidevineAuthorization =
            serde_json::from_slice(authorization_json).map_err(|error| error.to_string())?;
        if authorization.expires_unix <= now_unix {
            return Err("Widevine authorization has expired".into());
        }
        if !authorization.allowed_os.iter().any(|value| value == std::env::consts::OS)
            || !authorization
                .allowed_arch
                .iter()
                .any(|value| value == std::env::consts::ARCH)
        {
            return Err("Widevine authorization does not cover this target".into());
        }
        let component = fs::read(library).map_err(|error| error.to_string())?;
        let actual_hash = format!("{:x}", Sha256::digest(&component));
        if !actual_hash.eq_ignore_ascii_case(&authorization.component_sha256) {
            return Err("Widevine component hash is not authorized".into());
        }
        let signature = STANDARD
            .decode(&authorization.signature_base64)
            .map_err(|error| error.to_string())?;
        if !verifier.verify(&authorization.signed_payload()?, &signature) {
            return Err("Widevine authorization signature is invalid".into());
        }
        Ok(Self {
            library: library.to_path_buf(),
            manifest,
            authorization,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CdmRequest {
    Initialize {
        key_system: String,
        component_path: PathBuf,
        host_version: u32,
    },
    CreateSession {
        promise_id: u64,
        session_type: String,
        init_data_type: String,
        init_data: Vec<u8>,
    },
    UpdateSession {
        promise_id: u64,
        session_id: String,
        response: Vec<u8>,
    },
    CloseSession {
        promise_id: u64,
        session_id: String,
    },
    Decrypt {
        request_id: u64,
        key_id: Vec<u8>,
        iv: Vec<u8>,
        encrypted: Vec<u8>,
        subsamples: Vec<(u32, u32)>,
    },
    Shutdown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum CdmResponse {
    Initialized { module_version: String },
    PromiseResolved { promise_id: u64, value: Option<String> },
    PromiseRejected { promise_id: u64, reason: String },
    SessionMessage { session_id: String, message_type: String, message: Vec<u8> },
    KeyStatusesChanged { session_id: String, statuses: Vec<(Vec<u8>, String)> },
    Decrypted { request_id: u64, clear: Vec<u8> },
    Error { request_id: Option<u64>, reason: String },
}
