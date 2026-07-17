use crate::manifest::{NormalizedManifest, normalize};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Seek};
use zip::ZipArchive;

const MAX_PACKAGE_BYTES: usize = 512 * 1024 * 1024;
const MAX_CRX3_HEADER_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy)]
enum HashAlgorithm { Sha1, Sha256 }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PackageKind {
    FirefoxXpi,
    ChromeCrx3,
    DeveloperZip,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
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
        crx_header: &[u8],
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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
    if header_len == 0 || header_len > MAX_CRX3_HEADER_BYTES {
        return Err("CRX3 header size is invalid".into());
    }
    let payload_offset = 12usize.checked_add(header_len).ok_or("CRX header overflow")?;
    if payload_offset >= bytes.len() || !bytes[payload_offset..].starts_with(b"PK\x03\x04") {
        return Err("CRX3 ZIP payload is missing".into());
    }
    if bytes[12..payload_offset].windows(4)
        .any(|value| value == b"PK\x05\x06" || value == b"PK\x06\x07") {
        return Err("CRX3 header contains a ZIP end marker".into());
    }
    let identity = verifier.verify_crx3(&bytes[12..payload_offset], &bytes[payload_offset..])?;
    let mut manifest = read_manifest(&bytes[payload_offset..])?;
    manifest.chrome_compatibility_required = true;
    if manifest.manifest_version == 3 {
        manifest.warnings.push("CRX3 MV3 manifest normalized to shared extension semantics".into());
    }
    if let Some(key) = manifest.raw.get("key").and_then(serde_json::Value::as_str) {
        let public_key = STANDARD.decode(key).map_err(|_| "manifest key is not valid base64")?;
        if chrome_extension_id(&public_key) != identity.extension_id {
            return Err("CRX3 signer does not match manifest key".into());
        }
    }
    Ok(VerifiedPackage { kind: PackageKind::ChromeCrx3, sha256, identity: Some(identity), manifest })
}

pub fn chrome_extension_id(public_key: &[u8]) -> String {
    Sha256::digest(public_key)[..16].iter().flat_map(|byte| [byte >> 4, byte & 15])
        .map(|nibble| char::from(b'a' + nibble)).collect()
}

fn inspect_xpi_or_zip(
    bytes: &[u8],
    sha256: String,
    policy: &InstallPolicy,
    verifier: &dyn PackageSignatureVerifier,
) -> Result<VerifiedPackage, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    let signature_name = find_unique_metadata_entry(&mut archive, MetadataEntry::SignatureBlock)?;
    let identity = if let Some(signature_name) = signature_name {
        let manifest_name = find_required_metadata_entry(&mut archive, MetadataEntry::Manifest)?;
        let signature_file_name = find_required_metadata_entry(&mut archive, MetadataEntry::SignatureFile)?;
        let signature_block = read_required(&mut archive, &signature_name)?;
        let manifest_mf = read_required(&mut archive, &manifest_name)?;
        let signature_file = read_required(&mut archive, &signature_file_name)?;
        let ignored = BTreeSet::from([manifest_name, signature_file_name, signature_name]);
        verify_jar_integrity(&mut archive, &manifest_mf, &signature_file, &ignored)?;
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

fn verify_jar_integrity<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest_bytes: &[u8],
    signature_file: &[u8],
    ignored: &BTreeSet<String>,
) -> Result<(), String> {
    let signature = parse_manifest(signature_file)?;
    let signature_main = signature.first().ok_or("XPI signature file has no main section")?;
    verify_declared_digest(signature_main, "Digest-Manifest", manifest_bytes)?;

    let manifest = parse_manifest(manifest_bytes)?;
    let mut expected = BTreeMap::new();
    for section in manifest.iter().skip(1) {
        let name = field(section, "Name").ok_or("XPI manifest section has no Name")?;
        validate_archive_name(name)?;
        if expected.insert(name.to_string(), section).is_some() {
            return Err(format!("XPI manifest lists {name} more than once"));
        }
    }
    let mut seen = BTreeSet::new();
    let mut unpacked_bytes = 0u64;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name().to_string();
        validate_archive_name(&name)?;
        if !seen.insert(name.clone()) { return Err(format!("XPI contains duplicate entry {name}")); }
        if file.is_dir() || ignored.contains(&name) { continue; }
        if file.size() > MAX_ENTRY_BYTES { return Err(format!("XPI entry is too large: {name}")); }
        unpacked_bytes = unpacked_bytes.checked_add(file.size()).ok_or("XPI size overflow")?;
        if unpacked_bytes > MAX_UNPACKED_BYTES { return Err("XPI expands beyond size limit".into()); }
        let section = expected.remove(&name)
            .ok_or_else(|| format!("XPI entry is not signed: {name}"))?;
        verify_declared_reader(section, "Digest", &mut file)?;
    }
    if let Some((name, _)) = expected.into_iter().next() {
        return Err(format!("XPI manifest lists missing entry {name}"));
    }
    Ok(())
}

fn verify_declared_digest(
    fields: &BTreeMap<String, String>,
    suffix: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let (algorithm, expected) = declared_digest(fields, suffix)?;
    let actual = match algorithm {
        HashAlgorithm::Sha256 => STANDARD.encode(Sha256::digest(bytes)),
        HashAlgorithm::Sha1 => STANDARD.encode(Sha1::digest(bytes)),
    };
    if expected != actual { return Err(format!("XPI {suffix} digest mismatch")); }
    Ok(())
}

fn verify_declared_reader(
    fields: &BTreeMap<String, String>,
    suffix: &str,
    reader: &mut impl Read,
) -> Result<(), String> {
    let (algorithm, expected) = declared_digest(fields, suffix)?;
    let mut buffer = [0u8; 64 * 1024];
    let actual = match algorithm {
        HashAlgorithm::Sha256 => {
            let mut digest = Sha256::new();
            loop {
                let read = reader.read(&mut buffer).map_err(|error| error.to_string())?;
                if read == 0 { break; }
                digest.update(&buffer[..read]);
            }
            STANDARD.encode(digest.finalize())
        }
        HashAlgorithm::Sha1 => {
            let mut digest = Sha1::new();
            loop {
                let read = reader.read(&mut buffer).map_err(|error| error.to_string())?;
                if read == 0 { break; }
                digest.update(&buffer[..read]);
            }
            STANDARD.encode(digest.finalize())
        }
    };
    if expected != actual { return Err(format!("XPI {suffix} digest mismatch")); }
    Ok(())
}

fn declared_digest<'a>(
    fields: &'a BTreeMap<String, String>,
    suffix: &str,
) -> Result<(HashAlgorithm, &'a str), String> {
    if let Some(value) = field(fields, &format!("SHA-256-{suffix}")) {
        Ok((HashAlgorithm::Sha256, value))
    } else if let Some(value) = field(fields, &format!("SHA1-{suffix}")) {
        Ok((HashAlgorithm::Sha1, value))
    } else {
        Err(format!("XPI has no supported {suffix} digest"))
    }
}

fn parse_manifest(bytes: &[u8]) -> Result<Vec<BTreeMap<String, String>>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "XPI signature metadata is not UTF-8")?;
    let mut sections = Vec::new();
    let mut section = BTreeMap::new();
    let mut previous: Option<String> = None;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            if !section.is_empty() { sections.push(std::mem::take(&mut section)); }
            previous = None;
            continue;
        }
        if let Some(continuation) = line.strip_prefix(' ') {
            let key = previous.as_ref().ok_or("XPI metadata has orphan continuation line")?;
            section.get_mut(key).unwrap().push_str(continuation);
            continue;
        }
        let (key, value) = line.split_once(':').ok_or("XPI metadata line is malformed")?;
        let value = value.trim_start_matches(' ');
        if key.is_empty() || field(&section, key).is_some() {
            return Err("XPI metadata contains empty or duplicate field".into());
        }
        section.insert(key.into(), value.into());
        previous = Some(key.into());
    }
    if !section.is_empty() { sections.push(section); }
    if sections.is_empty() { return Err("XPI signature metadata is empty".into()); }
    Ok(sections)
}

fn field<'a>(fields: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    fields.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
}

fn validate_archive_name(name: &str) -> Result<(), String> {
    let path = name.strip_suffix('/').unwrap_or(name);
    if path.is_empty() || path.starts_with('/') || path.contains('\\') || path.contains(':') ||
        path.bytes().any(|byte| byte == 0 || byte < 0x20) || path.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || part.ends_with('.') ||
                part.ends_with(' ') ||
                is_windows_reserved(part.split('.').next().unwrap_or_default())
        }) {
        return Err("package contains unsafe archive path".into());
    }
    Ok(())
}

fn is_windows_reserved(value: &str) -> bool {
    let value = value.to_ascii_uppercase();
    matches!(value.as_str(), "CON" | "PRN" | "AUX" | "NUL") ||
        (value.len() == 4 && matches!(&value[..3], "COM" | "LPT") &&
            matches!(value.as_bytes()[3], b'1'..=b'9'))
}

#[derive(Clone, Copy)]
enum MetadataEntry { Manifest, SignatureFile, SignatureBlock }

fn find_required_metadata_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    kind: MetadataEntry,
) -> Result<String, String> {
    find_unique_metadata_entry(archive, kind)?.ok_or_else(||
        "XPI signature metadata is incomplete".into())
}

fn find_unique_metadata_entry<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    kind: MetadataEntry,
) -> Result<Option<String>, String> {
    let mut found = None;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name();
        if metadata_entry_matches(name, kind) {
            if found.replace(name.to_string()).is_some() {
                return Err("XPI has multiple matching signature metadata entries".into());
            }
        }
    }
    Ok(found)
}

fn metadata_entry_matches(name: &str, kind: MetadataEntry) -> bool {
    let Some((directory, file)) = name.rsplit_once('/') else { return false };
    if !directory.eq_ignore_ascii_case("META-INF") { return false; }
    match kind {
        MetadataEntry::Manifest => file.eq_ignore_ascii_case("manifest.mf"),
        MetadataEntry::SignatureFile => file.rsplit_once('.')
            .is_some_and(|(stem, suffix)| !stem.is_empty() && suffix.eq_ignore_ascii_case("sf")),
        MetadataEntry::SignatureBlock => file.rsplit_once('.')
            .is_some_and(|(stem, suffix)| !stem.is_empty() && suffix.eq_ignore_ascii_case("rsa")),
    }
}


fn read_manifest(bytes: &[u8]) -> Result<NormalizedManifest, String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    validate_archive(&mut archive)?;
    let data = read_required(&mut archive, "manifest.json")?;
    let value = serde_json::from_slice(&data).map_err(|error| error.to_string())?;
    normalize(value)
}

fn validate_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<(), String> {
    let mut names = BTreeSet::new();
    let mut unpacked_bytes = 0u64;
    for index in 0..archive.len() {
        let file = archive.by_index(index).map_err(|error| error.to_string())?;
        let name = file.name();
        validate_archive_name(name)?;
        if !names.insert(name.to_ascii_lowercase()) {
            return Err(format!("package contains duplicate entry {name}"));
        }
        if file.size() > MAX_ENTRY_BYTES {
            return Err(format!("package entry is too large: {name}"));
        }
        unpacked_bytes = unpacked_bytes.checked_add(file.size()).ok_or("package size overflow")?;
        if unpacked_bytes > MAX_UNPACKED_BYTES {
            return Err("package expands beyond size limit".into());
        }
    }
    Ok(())
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
