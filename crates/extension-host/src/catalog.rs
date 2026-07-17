use crate::manifest::{ActionSpec, NormalizedManifest, normalize};
use crate::assets::ExtensionAssetServer;
use crate::package::{PackageKind, VerifiedPackage};
use crate::runtime::{ExtensionId, ProfileScope};
use crate::worlds::WorldId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_UNPACKED_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ExtensionSummary {
    pub id: ExtensionId,
    pub name: String,
    pub version: String,
    pub enabled: bool,
    pub action: Option<ActionSpec>,
}

#[derive(Clone, Debug)]
pub struct RuntimeExtension {
    pub id: ExtensionId,
    pub base_url: String,
    pub manifest: NormalizedManifest,
}

#[derive(Clone, Debug)]
struct CatalogEntry {
    root: PathBuf,
    manifest: NormalizedManifest,
    enabled: bool,
    persistent: bool,
}

pub struct ExtensionCatalog {
    entries: BTreeMap<(ProfileScope, ExtensionId), CatalogEntry>,
    assets: Option<ExtensionAssetServer>,
}

impl Default for ExtensionCatalog {
    fn default() -> Self { Self { entries: BTreeMap::new(), assets: None } }
}

impl ExtensionCatalog {
    pub fn runtime_extensions(&self, profile: ProfileScope) -> Vec<RuntimeExtension> {
        let Some(assets) = &self.assets else { return Vec::new() };
        self.entries.iter().filter_map(|((scope, id), entry)| {
            if *scope != profile || !entry.enabled { return None; }
            let manifest_url = assets.url(&asset_key(profile, id), "manifest.json").ok()?;
            Some(RuntimeExtension {
                id: id.clone(),
                base_url: manifest_url.strip_suffix("manifest.json")?.to_owned(),
                manifest: entry.manifest.clone(),
            })
        }).collect()
    }

    pub fn resource(&self, profile: ProfileScope, id: &ExtensionId, path: &str) -> Result<Vec<u8>, String> {
        let entry = self.entries.get(&(profile, id.clone())).ok_or("extension is not installed")?;
        if !entry.enabled { return Err("extension is disabled".into()); }
        let relative = safe_resource_path(path)?;
        if entry.root.is_dir() {
            return fs::read(entry.root.join(&relative)).map_err(|error| error.to_string());
        }
        let file = fs::File::open(&entry.root).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
        let mut resource = archive.by_name(&relative).map_err(|error| error.to_string())?;
        if resource.size() > 16 * 1024 * 1024 { return Err("extension resource is too large".into()); }
        let mut bytes = Vec::with_capacity(resource.size() as usize);
        resource.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        Ok(bytes)
    }

    pub fn load_unpacked(
        &mut self,
        profile: ProfileScope,
        root: &Path,
        developer_mode: bool,
    ) -> Result<ExtensionSummary, String> {
        if !developer_mode { return Err("unpacked extensions require developer build".into()); }
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        if !root.is_dir() { return Err("unpacked extension root is not a directory".into()); }
        let manifest_path = root.join("manifest.json");
        let metadata = fs::metadata(&manifest_path).map_err(|error| error.to_string())?;
        if metadata.len() == 0 || metadata.len() > MAX_UNPACKED_MANIFEST_BYTES {
            return Err("extension manifest size is invalid".into());
        }
        let bytes = fs::read(&manifest_path).map_err(|error| error.to_string())?;
        let raw: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let manifest = normalize(raw)?;
        let id = ExtensionId(manifest.extension_id.clone().unwrap_or_else(|| {
            let digest = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
            format!("unpacked-{}@ubar.local", &digest[..32])
        }));
        let summary = summary(&id, &manifest, true);
        if self.assets.is_none() { self.assets = Some(ExtensionAssetServer::start()?); }
        self.assets.as_ref().unwrap().register_directory(
            &asset_key(profile, &id), root.clone(), manifest.raw.clone(),
        );
        self.entries.insert((profile, id), CatalogEntry {
            root, manifest, enabled: true, persistent: false,
        });
        Ok(summary)
    }

    pub fn list(&self, profile: ProfileScope) -> Vec<ExtensionSummary> {
        self.entries.iter().filter_map(|((scope, id), entry)| {
            (*scope == profile).then(|| summary(id, &entry.manifest, entry.enabled))
        }).collect()
    }

    pub fn install_package(
        &mut self,
        profile: ProfileScope,
        package_path: &Path,
        xpi_verifier: &Path,
        crx3_verifier: &Path,
        install_root: &Path,
    ) -> Result<ExtensionSummary, String> {
        if profile != ProfileScope::Normal {
            return Err("package install is disabled in private profiles".into());
        }
        let package_path = package_path.canonicalize().map_err(|error| error.to_string())?;
        if !package_path.is_file() { return Err("extension package is missing".into()); }
        let verified = verify_external(&package_path, xpi_verifier, crx3_verifier)?;
        if !matches!(verified.kind, PackageKind::FirefoxXpi | PackageKind::ChromeCrx3)
            || verified.identity.is_none() {
            return Err("package is not a signed XPI or CRX3 extension".into());
        }
        fs::create_dir_all(install_root).map_err(|error| error.to_string())?;
        let suffix = if verified.kind == PackageKind::ChromeCrx3 { "crx" } else { "xpi" };
        let root = install_root.join(format!("{}.{suffix}", verified.sha256));
        copy_verified_archive(&package_path, &root, &verified.sha256)?;
        let id = ExtensionId(verified.identity.as_ref().unwrap().extension_id.clone());
        let manifest = verified.manifest;
        let summary = summary(&id, &manifest, true);
        if self.assets.is_none() { self.assets = Some(ExtensionAssetServer::start()?); }
        self.assets.as_ref().unwrap().register_archive(
            &asset_key(profile, &id), root.clone(), manifest.raw.clone(),
        );
        self.entries.insert((profile, id), CatalogEntry {
            root, manifest, enabled: true, persistent: true,
        });
        Ok(summary)
    }

    pub fn load_installed(
        &mut self,
        profile: ProfileScope,
        xpi_verifier: &Path,
        crx3_verifier: &Path,
        install_root: &Path,
    ) -> Vec<String> {
        let Ok(entries) = fs::read_dir(install_root) else { return Vec::new() };
        let mut paths = entries.filter_map(Result::ok).map(|entry| entry.path())
            .filter(|path| path.is_file() && path.extension()
                .is_some_and(|value| value == "xpi" || value == "crx"))
            .collect::<Vec<_>>();
        paths.sort();
        let mut errors = Vec::new();
        for root in paths {
            let result = (|| -> Result<(), String> {
                let hash = root.file_stem().and_then(|value| value.to_str())
                    .ok_or("installed extension filename is not UTF-8")?;
                if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err("installed extension directory has invalid hash".into());
                }
                let verified = verify_external(&root, xpi_verifier, crx3_verifier)?;
                if !verified.sha256.eq_ignore_ascii_case(hash) || verified.identity.is_none() {
                    return Err("installed extension identity or hash changed".into());
                }
                let id = ExtensionId(verified.identity.as_ref().unwrap().extension_id.clone());
                let manifest = verified.manifest;
                if self.assets.is_none() { self.assets = Some(ExtensionAssetServer::start()?); }
                self.assets.as_ref().unwrap().register_archive(
                    &asset_key(profile, &id), root.clone(), manifest.raw.clone(),
                );
                self.entries.insert((profile, id), CatalogEntry {
                    root: root.clone(), manifest, enabled: true, persistent: true,
                });
                Ok(())
            })();
            if let Err(error) = result { errors.push(format!("{}: {error}", root.display())); }
        }
        errors
    }

    pub fn action(&self, profile: ProfileScope, id: &ExtensionId) -> Result<Value, String> {
        let entry = self.entries.get(&(profile, id.clone())).ok_or("extension is not installed")?;
        if !entry.enabled { return Err("extension is disabled".into()); }
        let action = entry.manifest.action.as_ref().ok_or("extension has no toolbar action")?;
        let popup = action.default_popup.as_ref()
            .map(|path| self.assets.as_ref().ok_or("extension asset server is unavailable")?
                .url(&asset_key(profile, id), path.trim_start_matches('/')))
            .transpose()?;
        Ok(serde_json::json!({
            "id": id,
            "title": action.default_title,
            "popup": popup,
            "root": entry.root,
        }))
    }

    pub fn remove(&mut self, profile: ProfileScope, id: &ExtensionId) -> bool {
        let removed = self.entries.remove(&(profile, id.clone()));
        if let Some(entry) = &removed {
            if let Some(assets) = &self.assets { assets.unregister(&asset_key(profile, id)); }
            if entry.persistent && entry.root.extension()
                .is_some_and(|value| value == "xpi" || value == "crx") {
                let _ = fs::remove_file(&entry.root);
            }
        }
        removed.is_some()
    }

    pub fn drop_private_profile(&mut self, private_id: u64) {
        self.drop_profile(ProfileScope::Private(private_id));
    }

    pub fn drop_profile(&mut self, profile: ProfileScope) {
        let ids = self.entries.keys().filter(|(scope, _)| *scope == profile)
            .map(|(_, id)| id.clone()).collect::<Vec<_>>();
        self.entries.retain(|(scope, _), _| *scope != profile);
        for id in ids {
            if let Some(assets) = &self.assets { assets.unregister(&asset_key(profile, &id)); }
        }
    }
}

fn safe_resource_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty() || path.starts_with('/') || path.split('/').any(|part| {
        part.is_empty() || part == "." || part == ".." || part.contains('\0')
    }) {
        return Err("extension resource path is invalid".into());
    }
    Ok(path)
}

fn summary(id: &ExtensionId, manifest: &NormalizedManifest, enabled: bool) -> ExtensionSummary {
    ExtensionSummary {
        id: id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        enabled,
        action: manifest.action.clone(),
    }
}

fn asset_key(profile: ProfileScope, id: &ExtensionId) -> String {
    match profile {
        ProfileScope::Normal => format!("n-{}", id.0),
        ProfileScope::Private(value) => format!("p{value}-{}", id.0),
    }
}

fn verify_external(
    package: &Path,
    xpi_verifier: &Path,
    crx3_verifier: &Path,
) -> Result<VerifiedPackage, String> {
    let mut magic = [0u8; 4];
    fs::File::open(package).and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| error.to_string())?;
    let verifier = match &magic {
        b"Cr24" => crx3_verifier,
        b"PK\x03\x04" => xpi_verifier,
        _ => return Err("extension package has unsupported magic".into()),
    }.canonicalize().map_err(|_| "required extension verifier is missing")?;
    let mut command = verifier_command(package, &verifier)?;
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() { "extension verifier rejected package".into() } else { error });
    }
    serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn verifier_command(package: &Path, verifier: &Path) -> Result<Command, String> {
    let mut command = Command::new("bwrap");
    command.args([
        "--die-with-parent", "--new-session", "--unshare-all", "--cap-drop", "ALL",
        "--clearenv", "--setenv", "HOME", "/nonexistent", "--tmpfs", "/tmp",
        "--proc", "/proc", "--dev", "/dev", "--dir", "/app", "--dir", "/input",
    ]);
    for system in ["/usr", "/lib", "/lib64"] {
        if Path::new(system).exists() { command.args(["--ro-bind", system, system]); }
    }
    command
        .arg("--ro-bind").arg(verifier).arg("/app/ubar-extension-verifier")
        .arg("--ro-bind").arg(package).arg("/input/package")
        .arg("--").arg("/app/ubar-extension-verifier").arg("/input/package");
    Ok(command)
}

#[cfg(target_os = "macos")]
fn verifier_command(package: &Path, verifier: &Path) -> Result<Command, String> {
    fn escape(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
    }
    let profile = format!(
        "(version 1)(deny default)(allow process-info* sysctl-read)\
         (allow file-read* (subpath \"/System\") (subpath \"/usr/lib\")\
         (literal \"{}\") (literal \"{}\"))",
        escape(verifier), escape(package),
    );
    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.env_clear().arg("-p").arg(profile).arg(verifier).arg(package);
    Ok(command)
}

#[cfg(target_os = "windows")]
fn verifier_command(package: &Path, verifier: &Path) -> Result<Command, String> {
    let broker = verifier.parent().ok_or("extension verifier has no parent directory")?
        .join("ubar-cdm-appcontainer.exe");
    if !broker.is_file() { return Err("AppContainer verifier broker is missing".into()); }
    let mut command = Command::new(broker);
    command
        .arg("--program").arg(verifier)
        .arg("--argument").arg(package)
        .arg("--read").arg(package)
        .arg("--memory-limit").arg((768u64 * 1024 * 1024).to_string());
    Ok(command)
}

fn copy_verified_archive(source: &Path, destination: &Path, expected_hash: &str) -> Result<(), String> {
    if destination.is_file() {
        let bytes = fs::read(destination).map_err(|error| error.to_string())?;
        return (format!("{:x}", Sha256::digest(&bytes)) == expected_hash)
            .then_some(()).ok_or("installed extension hash mismatch".into());
    }
    let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
    let mut input = fs::File::open(source).map_err(|error| error.to_string())?;
    let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
        .map_err(|error| error.to_string())?;
    let result = std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())
        .and_then(|_| output.sync_all().map_err(|error| error.to_string()))
        .and_then(|_| fs::rename(&temporary, destination).map_err(|error| error.to_string()));
    if result.is_err() { let _ = fs::remove_file(&temporary); }
    result
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "camelCase")]
pub enum ExtensionControlRequest {
    List,
    LoadUnpacked { directory: PathBuf },
    InstallPackage { path: PathBuf },
    GetAction { id: ExtensionId },
    Remove { id: ExtensionId },
    DispatchWorldApi {
        world: WorldId,
        capability: String,
        #[serde(rename = "requestId", alias = "request_id")]
        request_id: u64,
        namespace: String,
        member: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default, rename = "userGesture", alias = "user_gesture")]
        user_gesture: bool,
    },
}
