use serde::{Deserialize, Serialize};
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
}

impl Default for SettingsData {
    fn default() -> Self {
        Self {
            homepage_uri: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct BrowserState {
    pub settings: SettingsData,
    pub history: Vec<HistoryEntry>,
    pub bookmarks: Vec<BookmarkEntry>,
    #[serde(default)]
    pub open_tabs: Vec<String>,
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

    pub fn remove_bookmark(&mut self, uri: &str) -> bool {
        if let Some(index) = self.bookmarks.iter().position(|entry| entry.uri == uri) {
            self.bookmarks.remove(index);
            self.save();
            return true;
        }
        false
    }
}

pub fn asset_uri(relative: &str) -> String {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = exe_dir.join(relative);
    format!("file://{}", path.to_string_lossy())
}
