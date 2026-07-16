use crate::manifest::{NormalizedManifest, normalize};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};
use zip::ZipArchive;

const MAX_PACKAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PackageKind {
    FirefoxXpi,
    ChromeCrx3,
    DeveloperZip,
}

#[derive(Clone, Debug)]
pub struct VerifiedIdentity {
    pub extension_id: String,
    pub signer: String,
    pub certificate_fingerprint_sha256: String,
}

pub trait PackageSignatureVerifier {
    fn verify_xpi(
        &self,
        package: &[u8],
        manifest_mf: &[u8],
        signature_file: &[u8],
        signature_block: &[u8],
    ) -> Result<VerifiedIdentity, String>;

    fn verify_crx3(
        &self,
        signed_header: &[u8],
        zip_payload: &[u8],
    ) -> Result<VerifiedIdentity, String>;
}

pub struct RejectUnsigned;

impl PackageSignatureVerifier for RejectUnsigned {
    fn verify_xpi(&self, _: &[u8], _: &[u8], _: &[u8], _: &[u8]) -> Result<VerifiedIdentity, String> {
        Err("no Mozilla package-signature verifier is configured".into())
    }
    fn verify_crx3(&self, _: &[u8], _: &[u8]) -> Result<VerifiedIdentity, String> {
        Err("no CRX3 package-signature verifier is configured".into())
    }
}

#[derive(Clone, Debug)]
pub struct InstallPolicy {
    pub developer_mode: bool,
    pub require_store_signature: bool,
}

impl Default for InstallPolicy {
    fn default() -> Self {
        Self { developer_mode: false, require_store_signature: true }
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedPackage {
    pub kind: PackageKind,
    pub sha256: String,
    pub identity: Option<VerifiedIdentity>,
    pub manifest: NormalizedManifest,
}

pub fn inspect_package(
    bytes: &[u8],
    policy: &InstallPolicy,
    verifier: &dyn PackageSignatureVerifier,
) -> Result<VerifiedPackage, String> {
    if bytes.is_empty() || bytes.len() > MAX_PACKAGE_BYTES {
        return Err("extension package size is invalid".into());
    }
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    if bytes.starts_with(b"Cr24") {
        inspect_crx3(bytes, sha256, verifier)
    } else if bytes.starts_with(b"PK\x03\x04") {
        inspect_xpi_or_zip(bytes, sha256, policy, verifier)
    } else {
        Err("extension package is neither XPI/ZIP nor CRX3".into())
    }
}

fn inspect_crx3(
    bytes: &[u8],
    sha256: String,
    verifier: &dyn PackageSignatureVerifier,
) -> Result<VerifiedPackage, String> {
    if bytes.len() < 12 {
        return Err("CRX header is truncated".into());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != 3 {
        return Err(format!("unsupported CRX version {version}"));
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let payload_offset = 12usize.checked_add(header_len).ok_or("CRX header overflow")?;
    if payload_offset >= bytes.len() || !bytes[payload_offset..].starts_with(b"PK\x03\x04") {
        return Err("CRX3 ZIP payload is missing".into());
    }
    let identity = verifier.verify_crx3(&bytes[12..payload_offset], &bytes[payload_offset..])?;
    let manifest = read_manifest(&bytes[payload_offset..])?;
    Ok(VerifiedPackage { kind: PackageKind::ChromeCrx3, sha256, identity: Some(identity), manifest })
}

fn inspect_xpi_or_zip(
    bytes: &[u8],
    sha256: String,
    policy: &InstallPolicy,
    verifier: &dyn PackageSignatureVerifier,
) -> Result<VerifiedPackage, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let signature = match read_optional(&mut archive, "META-INF/mozilla.rsa")? {
        Some(value) => Some(value),
        None => read_optional(&mut archive, "META-INF/mozilla.ec")?,
    };
    let identity = if let Some(signature_block) = signature {
        let manifest_mf = read_required(&mut archive, "META-INF/manifest.mf")?;
        let signature_file = read_required(&mut archive, "META-INF/mozilla.sf")?;
        Some(verifier.verify_xpi(bytes, &manifest_mf, &signature_file, &signature_block)?)
    } else if policy.require_store_signature || !policy.developer_mode {
        return Err("unsigned extension packages require explicit developer mode".into());
    } else {
        None
    };
    drop(archive);
    let manifest = read_manifest(bytes)?;
    if let (Some(signed), Some(declared)) = (&identity, &manifest.extension_id) {
        if &signed.extension_id != declared {
            return Err("signed extension ID does not match manifest Gecko ID".into());
        }
    }
    Ok(VerifiedPackage {
        kind: if identity.is_some() { PackageKind::FirefoxXpi } else { PackageKind::DeveloperZip },
        sha256,
        identity,
        manifest,
    })
}

fn read_manifest(bytes: &[u8]) -> Result<NormalizedManifest, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let data = read_required(&mut archive, "manifest.json")?;
    let value = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    normalize(value)
}

fn read_required<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<Vec<u8>, String> {
    read_optional(archive, name)?.ok_or_else(|| format!("package has no {name}"))
}

fn read_optional<R: Read + std::io::Seek>(archive: &mut ZipArchive<R>, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Ok(mut file) = archive.by_name(name) else { return Ok(None) };
    if file.size() > MAX_MANIFEST_BYTES {
        return Err(format!("{name} exceeds size limit"));
    }
    let mut data = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut data).map_err(|error| error.to_string())?;
    Ok(Some(data))
}
