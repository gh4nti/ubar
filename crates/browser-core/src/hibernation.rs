use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"UBARHIB1";
const MAX_STATE_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HibernationSnapshot {
    pub view_id: u64,
    pub uri: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub history: Vec<String>,
    #[serde(default)]
    pub history_index: usize,
    #[serde(default)]
    pub scroll_x: i64,
    #[serde(default)]
    pub scroll_y: i64,
    #[serde(default)]
    pub page_state_base64: String,
    pub created_at_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotHeader {
    view_id: u64,
    uri: String,
    title: String,
    history: Vec<String>,
    history_index: usize,
    scroll_x: i64,
    scroll_y: i64,
    created_at_ms: u64,
    state_bytes: usize,
}

pub struct HibernationStore {
    directory: Option<PathBuf>,
    volatile: BTreeMap<u64, HibernationSnapshot>,
}

impl HibernationStore {
    pub fn persistent(profile_directory: &Path) -> Self {
        Self { directory: Some(profile_directory.join("hibernated-tabs")), volatile: BTreeMap::new() }
    }

    pub fn private() -> Self {
        Self { directory: None, volatile: BTreeMap::new() }
    }

    pub fn put(&mut self, snapshot: HibernationSnapshot) -> Result<(), String> {
        validate_snapshot(&snapshot)?;
        let Some(directory) = &self.directory else {
            self.volatile.insert(snapshot.view_id, snapshot);
            return Ok(());
        };
        let state = STANDARD.decode(snapshot.page_state_base64.as_bytes())
            .map_err(|_| "page state is not valid base64".to_owned())?;
        if state.len() > MAX_STATE_BYTES { return Err("page state exceeds 256 MiB".into()); }
        let header = SnapshotHeader {
            view_id: snapshot.view_id, uri: snapshot.uri, title: snapshot.title,
            history: snapshot.history, history_index: snapshot.history_index,
            scroll_x: snapshot.scroll_x, scroll_y: snapshot.scroll_y,
            created_at_ms: snapshot.created_at_ms, state_bytes: state.len(),
        };
        let json = serde_json::to_vec(&header).map_err(|error| error.to_string())?;
        let mut encoded = Vec::with_capacity(MAGIC.len() + 8 + json.len() + state.len());
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&(json.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&json);
        encoded.extend_from_slice(&state);
        fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        atomic_replace(&directory.join(format!("{}.hib", header.view_id)), &encoded)
    }

    pub fn take(&mut self, view_id: u64) -> Result<Option<HibernationSnapshot>, String> {
        let Some(directory) = &self.directory else { return Ok(self.volatile.remove(&view_id)); };
        let path = directory.join(format!("{view_id}.hib"));
        if !path.is_file() { return Ok(None); }
        let encoded = fs::read(&path).map_err(|error| error.to_string())?;
        let snapshot = decode(&encoded)?;
        fs::remove_file(path).map_err(|error| error.to_string())?;
        Ok(Some(snapshot))
    }

    pub fn remove(&mut self, view_id: u64) -> Result<bool, String> {
        let Some(directory) = &self.directory else { return Ok(self.volatile.remove(&view_id).is_some()); };
        let path = directory.join(format!("{view_id}.hib"));
        if !path.is_file() { return Ok(false); }
        fs::remove_file(path).map_err(|error| error.to_string())?;
        Ok(true)
    }
}

fn validate_snapshot(snapshot: &HibernationSnapshot) -> Result<(), String> {
    if snapshot.view_id == 0 || snapshot.uri.len() > 32 * 1024
        || !(snapshot.uri == "about:blank" || snapshot.uri.starts_with("http://") || snapshot.uri.starts_with("https://"))
        || snapshot.history.len() > 256 || snapshot.history_index > snapshot.history.len().saturating_sub(1)
    {
        return Err("invalid hibernation snapshot".into());
    }
    Ok(())
}

fn decode(encoded: &[u8]) -> Result<HibernationSnapshot, String> {
    if encoded.len() < 16 || &encoded[..8] != MAGIC { return Err("invalid hibernation snapshot".into()); }
    let json_len = u64::from_le_bytes(encoded[8..16].try_into().unwrap()) as usize;
    if json_len > 1024 * 1024 || 16 + json_len > encoded.len() { return Err("invalid hibernation header".into()); }
    let header: SnapshotHeader = serde_json::from_slice(&encoded[16..16 + json_len]).map_err(|error| error.to_string())?;
    let state = &encoded[16 + json_len..];
    if state.len() != header.state_bytes || state.len() > MAX_STATE_BYTES { return Err("invalid hibernation state length".into()); }
    Ok(HibernationSnapshot {
        view_id: header.view_id, uri: header.uri, title: header.title, history: header.history,
        history_index: header.history_index, scroll_x: header.scroll_x, scroll_y: header.scroll_y,
        page_state_base64: STANDARD.encode(state), created_at_ms: header.created_at_ms,
    })
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if path.exists() {
        let backup = path.with_extension("previous");
        let _ = fs::remove_file(&backup);
        fs::rename(path, &backup).map_err(|error| error.to_string())?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::rename(&backup, path);
            return Err(error.to_string());
        }
        let _ = fs::remove_file(backup);
    } else {
        fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    }
    Ok(())
}
