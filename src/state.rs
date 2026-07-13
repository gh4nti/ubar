use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct HistoryEntry {
    pub title: String,
    pub uri: String,
    pub timestamp: i64,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BookmarkEntry {
    pub title: String,
    pub uri: String,
    pub timestamp: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SettingsData {
    pub homepage_uri: String,
    #[serde(default = "default_search_engine")]
    pub search_engine: String,
    #[serde(default)]
    pub download_dir: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_zoom")]
    pub default_zoom: f64,
    #[serde(default = "default_font_size")]
    pub font_size: u32,
}

fn default_search_engine() -> String {
    "duckduckgo".into()
}

fn default_theme() -> String {
    "system".into()
}

fn default_zoom() -> f64 {
    1.0
}

fn default_font_size() -> u32 {
    16
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            homepage_uri: String::new(),
            search_engine: default_search_engine(),
            download_dir: String::new(),
            theme: default_theme(),
            default_zoom: default_zoom(),
            font_size: default_font_size(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct DownloadEntry {
    pub id: u64,
    pub uri: String,
    pub destination: String,
    pub filename: String,
    pub received: u64,
    pub total: u64,
    pub status: String,
    pub timestamp: i64,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BrowserState {
    pub settings: SettingsData,
    pub history: Vec<HistoryEntry>,
    pub bookmarks: Vec<BookmarkEntry>,
    #[serde(default)]
    pub open_tabs: Vec<String>,
    #[serde(default)]
    pub permission_defaults: BTreeMap<String, String>,
    #[serde(default)]
    pub site_permissions: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub downloads: Vec<DownloadEntry>,
    #[serde(default)]
    pub download_counter: u64,
    #[serde(skip)]
    pub path: PathBuf,
}

impl BrowserState {
    pub fn load() -> Self {
        let path = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ubar")
            .join("state.json");

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let mut state = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<BrowserState>(&text).ok())
            .unwrap_or_default();
        state.path = path;
        state
    }

    pub fn save(&self) {
        if self.path.as_os_str().is_empty() {
            return;
        }

        let tmp = self.path.with_extension("json.tmp");
        if let Ok(data) = serde_json::to_vec_pretty(self) {
            let _ = fs::write(&tmp, data);
            let _ = fs::rename(&tmp, &self.path);
        }
    }

    pub fn add_history(&mut self, title: &str, uri: &str, timestamp: i64) {
        if uri.is_empty() {
            return;
        }

        if let Some(entry) = self.history.first_mut()
            && entry.uri == uri
        {
            entry.title = if title.is_empty() { uri.into() } else { title.into() };
            entry.timestamp = timestamp;
            self.save();
            return;
        }

        self.history.insert(
            0,
            HistoryEntry {
                title: if title.is_empty() { uri.into() } else { title.into() },
                uri: uri.into(),
                timestamp,
            },
        );
        self.save();
    }

    #[cfg(not(target_os = "windows"))]
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.save();
    }

    pub fn is_bookmarked(&self, uri: &str) -> bool {
        self.bookmarks.iter().any(|entry| entry.uri == uri)
    }

    pub fn toggle_bookmark(&mut self, title: &str, uri: &str, timestamp: i64) -> bool {
        if let Some(index) = self.bookmarks.iter().position(|entry| entry.uri == uri) {
            self.bookmarks.remove(index);
            self.save();
            return false;
        }

        self.bookmarks.insert(
            0,
            BookmarkEntry {
                title: if title.is_empty() { uri.into() } else { title.into() },
                uri: uri.into(),
                timestamp,
            },
        );
        self.save();
        true
    }

    // "ask" unless a site rule or default says otherwise.
    #[cfg(not(target_os = "windows"))]
    pub fn permission_for(&self, origin: &str, key: &str) -> String {
        self.site_permissions
            .get(origin)
            .and_then(|rules| rules.get(key))
            .or_else(|| self.permission_defaults.get(key))
            .cloned()
            .unwrap_or_else(|| "ask".into())
    }

    #[cfg(not(target_os = "windows"))]
    pub fn set_site_permission(&mut self, origin: &str, key: &str, value: &str) {
        self.site_permissions
            .entry(origin.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
        self.save();
    }

    pub fn add_download(&mut self, uri: &str, destination: &str, filename: &str, timestamp: i64) -> u64 {
        self.download_counter += 1;
        let id = self.download_counter;
        self.downloads.insert(
            0,
            DownloadEntry {
                id,
                uri: uri.into(),
                destination: destination.into(),
                filename: filename.into(),
                received: 0,
                total: 0,
                status: "active".into(),
                timestamp,
            },
        );
        self.save();
        id
    }

    pub fn download_mut(&mut self, id: u64) -> Option<&mut DownloadEntry> {
        self.downloads.iter_mut().find(|entry| entry.id == id)
    }

    #[cfg(not(target_os = "windows"))]
    pub fn remove_bookmark(&mut self, uri: &str) -> bool {
        if let Some(index) = self.bookmarks.iter().position(|entry| entry.uri == uri) {
            self.bookmarks.remove(index);
            self.save();
            return true;
        }
        false
    }
}

#[cfg(not(target_os = "windows"))]
pub fn asset_uri(relative: &str) -> String {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = exe_dir.join(relative);
    format!("file://{}", path.to_string_lossy())
}
