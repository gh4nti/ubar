use crate::hibernation::{HibernationSnapshot, HibernationStore};
use crate::secrets::SecretStore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_HISTORY: usize = 50_000;
const MAX_SETTING_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus { Pending, Active, Paused, Complete, Cancelled, Failed }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRecord {
    pub id: String,
    pub url: String,
    pub destination: String,
    pub suggested_filename: String,
    pub status: DownloadStatus,
    pub received_bytes: u64,
    pub total_bytes: Option<u64>,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    #[serde(default)] pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark { pub id: String, pub title: String, pub url: String, pub folder: String, pub created_at_ms: u64 }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry { pub id: String, pub title: String, pub url: String, pub visited_at_ms: u64, pub transition: String }

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionDecision { Ask, Allow, Block }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SitePermission { pub origin: String, pub permission: String, pub decision: PermissionDecision, pub updated_at_ms: u64 }

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetadata { pub id: String, pub origin: String, pub username: String, pub created_at_ms: u64, pub updated_at_ms: u64, secret_ref: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedState {
    #[serde(default = "one")] next_id: u64,
    #[serde(default)] downloads: BTreeMap<String, DownloadRecord>,
    #[serde(default)] bookmarks: BTreeMap<String, Bookmark>,
    #[serde(default)] history: Vec<HistoryEntry>,
    #[serde(default)] settings: BTreeMap<String, Value>,
    #[serde(default)] permissions: BTreeMap<String, SitePermission>,
    #[serde(default)] credentials: BTreeMap<String, CredentialMetadata>,
}

fn one() -> u64 { 1 }
impl Default for PersistedState {
    fn default() -> Self { Self { next_id: 1, downloads: BTreeMap::new(), bookmarks: BTreeMap::new(), history: Vec::new(), settings: BTreeMap::new(), permissions: BTreeMap::new(), credentials: BTreeMap::new() } }
}

pub struct ProfileServices {
    persistent: bool,
    path: Option<PathBuf>,
    state: PersistedState,
    hibernation: HibernationStore,
    secrets: Arc<dyn SecretStore>,
}

impl ProfileServices {
    pub fn open_persistent(directory: &Path, secrets: Arc<dyn SecretStore>) -> Result<Self, String> {
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let path = directory.join("browser-services.json");
        let state = if path.is_file() { serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?).map_err(|error| format!("invalid browser services state: {error}"))? } else { PersistedState::default() };
        Ok(Self { persistent: true, path: Some(path), state, hibernation: HibernationStore::persistent(directory), secrets })
    }

    pub fn private(secrets: Arc<dyn SecretStore>) -> Self {
        Self { persistent: false, path: None, state: PersistedState::default(), hibernation: HibernationStore::private(), secrets }
    }

    pub fn control(&mut self, request: BrowserControlRequest) -> Result<Value, String> {
        match request {
            BrowserControlRequest::ListDownloads => value(self.state.downloads.values().collect::<Vec<_>>()),
            BrowserControlRequest::PutDownload { download } => { validate_download(&download)?; self.change(|state| { state.downloads.insert(download.id.clone(), download.clone()); }); self.persist()?; value(download) }
            BrowserControlRequest::RemoveDownload { id } => { let removed = self.state.downloads.remove(&id).is_some(); self.persist()?; Ok(json!({"removed": removed})) }
            BrowserControlRequest::ListBookmarks => value(self.state.bookmarks.values().collect::<Vec<_>>()),
            BrowserControlRequest::AddBookmark { title, url, folder, created_at_ms } => {
                validate_url(&url)?; limited(&title, 4096, "bookmark title")?; limited(&folder, 4096, "bookmark folder")?;
                let id = self.next_id("bookmark"); let bookmark = Bookmark { id: id.clone(), title, url, folder, created_at_ms };
                self.state.bookmarks.insert(id, bookmark.clone()); self.persist()?; value(bookmark)
            }
            BrowserControlRequest::RemoveBookmark { id } => { let removed = self.state.bookmarks.remove(&id).is_some(); self.persist()?; Ok(json!({"removed": removed})) }
            BrowserControlRequest::QueryHistory { limit } => { let limit = limit.unwrap_or(500).min(10_000); value(self.state.history.iter().rev().take(limit).collect::<Vec<_>>()) }
            BrowserControlRequest::RecordHistory { title, url, visited_at_ms, transition } => {
                if !self.persistent { return Ok(Value::Null); }
                validate_url(&url)?; limited(&title, 4096, "history title")?; limited(&transition, 64, "transition")?;
                let entry = HistoryEntry { id: self.next_id("history"), title, url, visited_at_ms, transition };
                self.state.history.push(entry.clone()); if self.state.history.len() > MAX_HISTORY { self.state.history.drain(..self.state.history.len() - MAX_HISTORY); }
                self.persist()?; value(entry)
            }
            BrowserControlRequest::ClearHistory => { self.state.history.clear(); self.persist()?; Ok(json!({"cleared": true})) }
            BrowserControlRequest::GetSettings => value(&self.state.settings),
            BrowserControlRequest::SetSetting { key, value: setting } => {
                validate_key(&key)?; if serde_json::to_vec(&setting).map_err(|error| error.to_string())?.len() > MAX_SETTING_BYTES { return Err("setting exceeds 64 KiB".into()); }
                self.state.settings.insert(key, setting); self.persist()?; Ok(json!({"saved": true}))
            }
            BrowserControlRequest::DeleteSetting { key } => { let removed = self.state.settings.remove(&key).is_some(); self.persist()?; Ok(json!({"removed": removed})) }
            BrowserControlRequest::ListPermissions => value(self.state.permissions.values().collect::<Vec<_>>()),
            BrowserControlRequest::SetPermission { origin, permission, decision, updated_at_ms } => {
                let origin = normalize_origin(&origin)?; validate_key(&permission)?;
                let record = SitePermission { origin: origin.clone(), permission: permission.clone(), decision, updated_at_ms };
                self.state.permissions.insert(format!("{origin}\n{permission}"), record.clone()); self.persist()?; value(record)
            }
            BrowserControlRequest::ClearPermission { origin, permission } => {
                let origin = normalize_origin(&origin)?; let removed = self.state.permissions.remove(&format!("{origin}\n{permission}")).is_some(); self.persist()?; Ok(json!({"removed": removed}))
            }
            BrowserControlRequest::ListCredentials => value(self.state.credentials.values().collect::<Vec<_>>()),
            BrowserControlRequest::SaveCredential { origin, username, password, now_ms } => self.save_credential(origin, username, password, now_ms),
            BrowserControlRequest::GetCredential { id } => {
                let metadata = self.state.credentials.get(&id).ok_or("credential not found")?;
                let password = self.secrets.get(&metadata.secret_ref).map_err(|error| error.to_string())?;
                Ok(json!({"metadata": metadata, "password": password}))
            }
            BrowserControlRequest::RemoveCredential { id } => {
                let Some(metadata) = self.state.credentials.get(&id).cloned() else { return Ok(json!({"removed": false})); };
                let previous = self.secrets.get(&metadata.secret_ref).map_err(|error| error.to_string())?;
                self.secrets.delete(&metadata.secret_ref).map_err(|error| error.to_string())?;
                self.state.credentials.remove(&id);
                if let Err(error) = self.persist() {
                    self.state.credentials.insert(id, metadata.clone());
                    let _ = self.secrets.set(&metadata.secret_ref, &previous);
                    return Err(error);
                }
                Ok(json!({"removed": true}))
            }
            BrowserControlRequest::Hibernate { snapshot } => { self.hibernation.put(snapshot)?; Ok(json!({"stored": true})) }
            BrowserControlRequest::RestoreHibernation { view_id } => value(self.hibernation.take(view_id)?),
            BrowserControlRequest::RemoveHibernation { view_id } => Ok(json!({"removed": self.hibernation.remove(view_id)?})),
            BrowserControlRequest::ReportMemory { resident_bytes } => Ok(json!({"residentBytes": resident_bytes})),
        }
    }

    fn save_credential(&mut self, origin: String, username: String, password: String, now_ms: u64) -> Result<Value, String> {
        if !self.persistent { return Err("private profiles cannot save credentials".into()); }
        let origin = normalize_origin(&origin)?; limited(&username, 4096, "username")?; limited(&password, 64 * 1024, "password")?;
        let existing = self.state.credentials.values().find(|item| item.origin == origin && item.username == username).cloned();
        let previous_secret = existing.as_ref().and_then(|item| self.secrets.get(&item.secret_ref).ok());
        let (id, secret_ref, created_at_ms) = existing.map_or_else(|| { let id = self.next_id("credential"); (id.clone(), format!("credential:{id}"), now_ms) }, |item| (item.id, item.secret_ref, item.created_at_ms));
        self.secrets.set(&secret_ref, &password).map_err(|error| error.to_string())?;
        let metadata = CredentialMetadata { id: id.clone(), origin, username, created_at_ms, updated_at_ms: now_ms, secret_ref };
        let prior_metadata = self.state.credentials.insert(id.clone(), metadata.clone());
        if let Err(error) = self.persist() {
            match prior_metadata { Some(value) => { self.state.credentials.insert(id, value); }, None => { self.state.credentials.remove(&id); } }
            if let Some(secret) = previous_secret { let _ = self.secrets.set(&metadata.secret_ref, &secret); }
            else { let _ = self.secrets.delete(&metadata.secret_ref); }
            return Err(error);
        }
        value(metadata)
    }

    fn next_id(&mut self, prefix: &str) -> String { let id = self.state.next_id; self.state.next_id = self.state.next_id.saturating_add(1); format!("{prefix}-{id}") }
    fn change(&mut self, operation: impl FnOnce(&mut PersistedState)) { operation(&mut self.state); }
    fn persist(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()); };
        let encoded = serde_json::to_vec(&self.state).map_err(|error| error.to_string())?;
        atomic_replace(path, &encoded)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "camelCase")]
pub enum BrowserControlRequest {
    ListDownloads,
    PutDownload { download: DownloadRecord },
    RemoveDownload { id: String },
    ListBookmarks,
    AddBookmark { title: String, url: String, #[serde(default)] folder: String, created_at_ms: u64 },
    RemoveBookmark { id: String },
    QueryHistory { limit: Option<usize> },
    RecordHistory { title: String, url: String, visited_at_ms: u64, #[serde(default = "default_transition")] transition: String },
    ClearHistory,
    GetSettings,
    SetSetting { key: String, value: Value },
    DeleteSetting { key: String },
    ListPermissions,
    SetPermission { origin: String, permission: String, decision: PermissionDecision, updated_at_ms: u64 },
    ClearPermission { origin: String, permission: String },
    ListCredentials,
    SaveCredential { origin: String, username: String, password: String, now_ms: u64 },
    GetCredential { id: String },
    RemoveCredential { id: String },
    Hibernate { snapshot: HibernationSnapshot },
    RestoreHibernation { view_id: u64 },
    RemoveHibernation { view_id: u64 },
    ReportMemory { resident_bytes: u64 },
}

fn default_transition() -> String { "link".into() }
fn value(value: impl Serialize) -> Result<Value, String> { serde_json::to_value(value).map_err(|error| error.to_string()) }
fn validate_key(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > 128 || !value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)) { Err("invalid key".into()) } else { Ok(()) }
}
fn limited(value: &str, maximum: usize, label: &str) -> Result<(), String> { if value.len() > maximum { Err(format!("{label} too long")) } else { Ok(()) } }
fn validate_url(url: &str) -> Result<(), String> {
    if url.len() <= 32 * 1024 && (url.starts_with("https://") || url.starts_with("http://")) { Ok(()) } else { Err("invalid HTTP URL".into()) }
}
fn validate_download(download: &DownloadRecord) -> Result<(), String> {
    validate_key(&download.id)?; validate_url(&download.url)?; limited(&download.destination, 32 * 1024, "download destination")?; limited(&download.suggested_filename, 4096, "download filename")?;
    if download.total_bytes.is_some_and(|total| download.received_bytes > total) { return Err("received bytes exceed total".into()); }
    Ok(())
}
fn normalize_origin(origin: &str) -> Result<String, String> {
    let (scheme, rest) = origin.split_once("://").ok_or("invalid origin")?;
    if !matches!(scheme, "http" | "https") || rest.is_empty() || rest.len() > 2048 || rest.contains(&['@', ' ', '\t', '\r', '\n', '?', '#'][..]) { return Err("invalid origin".into()); }
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains('/') { return Err("origin must not contain a path".into()); }
    Ok(format!("{}://{}", scheme.to_ascii_lowercase(), authority.to_ascii_lowercase()))
}
fn atomic_replace(path: &Path, encoded: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, encoded).map_err(|error| error.to_string())?;
    if path.exists() {
        let backup = path.with_extension("previous"); let _ = fs::remove_file(&backup);
        fs::rename(path, &backup).map_err(|error| error.to_string())?;
        if let Err(error) = fs::rename(&temporary, path) { let _ = fs::rename(&backup, path); return Err(error.to_string()); }
        let _ = fs::remove_file(backup);
    } else { fs::rename(&temporary, path).map_err(|error| error.to_string())?; }
    Ok(())
}
