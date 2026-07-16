use crate::state::BrowserState;
use chrono::Utc;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
use webview2_com::TrySuspendCompletedHandler;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use windows::core::Interface;
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos},
};
use wry::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{
    PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows,
};

const TOOLBAR_HEIGHT: f64 = 82.0;
const MENU_OVERLAY_HEIGHT: f64 = 360.0;
const NEW_TAB_URI: &str = "ubar://localhost/newtab/index.html";
const TOOLBAR_URI: &str = "ubar://localhost/windows/index.html";
const HISTORY_URI: &str = "ubar://localhost/pages/history/index.html";
const BOOKMARKS_URI: &str = "ubar://localhost/pages/bookmarks/index.html";
const DOWNLOADS_URI: &str = "ubar://localhost/pages/downloads/index.html";
const EXTENSIONS_URI: &str = "ubar://localhost/pages/extensions/index.html";
const SETTINGS_URI: &str = "ubar://localhost/pages/settings/index.html";

#[derive(Debug)]
enum UserEvent {
    Toolbar(String),
    Content(u64, String),
    Loaded(u64, String),
    Title(u64, String),
    SuspendCompleted(u64, bool),
    ExtensionsChanged,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TabLifecycle {
    Active,
    SuspendPending,
    Suspended,
}

struct Tab {
    id: u64,
    webview: WebView,
    uri: String,
    title: String,
    incognito: bool,
    zoom: f64,
    lifecycle: TabLifecycle,
    loaded: bool,
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    toolbar: Option<WebView>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
    menu_open: bool,
    state: Rc<RefCell<BrowserState>>,
    new_tab_uri: String,
    memory_policy: crate::memory_policy::MemoryPolicy,
    private_window: bool,
    benchmark_started: Option<std::time::Instant>,
    benchmark_urls: Vec<String>,
    benchmark_next: usize,
    benchmark_loaded: HashSet<u64>,
    benchmark_settled_at: Option<std::time::Instant>,
    benchmark_trimmed_at: Option<std::time::Instant>,
    benchmark_restore_started: Option<std::time::Instant>,
    benchmark_restore_completed: Option<std::time::Instant>,
    benchmark_reported: bool,
}

fn benchmark_urls() -> Vec<String> {
    if let Ok(value) = std::env::var("UBAR_BENCHMARK_URLS") {
        let urls = value
            .split(',')
            .map(str::trim)
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if urls.len() == 5 {
            return urls;
        }
    }
    [
        "https://github.com/",
        "https://www.reddit.com/",
        "https://www.wikipedia.org/",
        "https://www.microsoft.com/",
        "https://www.apple.com/",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn normalize_uri(input: &str, engine: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if input.contains("://") || input.starts_with("file:") {
        return Some(input.to_string());
    }
    if input.contains(' ') || (!input.contains('.') && !input.contains(':')) {
        let query = input
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (byte as char).to_string()
                }
                b' ' => "+".into(),
                _ => format!("%{byte:02X}"),
            })
            .collect::<String>();
        return Some(match engine {
            "google" => format!("https://www.google.com/search?q={query}"),
            "bing" => format!("https://www.bing.com/search?q={query}"),
            "yandex" => format!("https://yandex.com/search/?text={query}"),
            _ => format!("https://duckduckgo.com/?q={query}"),
        });
    }
    Some(format!("https://{input}"))
}

fn content_script() -> &'static str {
    r#"
window.addEventListener('keydown', event => {
  if (event.key === 'F12') {
    event.preventDefault();
    window.ipc.postMessage(JSON.stringify({cmd:'devtools'}));
    return;
  }
  if (!event.ctrlKey) return;
  const key = event.key.toLowerCase();
  if (['l','t','w','r','d','+','=','-','0'].includes(key)) {
    event.preventDefault();
    window.ipc.postMessage(JSON.stringify({cmd:'shortcut',key}));
  }
});
window.webkit = window.webkit || {};
window.webkit.messageHandlers = window.webkit.messageHandlers || {};
window.webkit.messageHandlers.ubar = {
  postMessage(message) {
    window.ipc.postMessage(JSON.stringify({cmd:'internal', message:String(message)}));
  }
};
"#
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                (bytes[index + 1] as char).to_digit(16),
                (bytes[index + 2] as char).to_digit(16),
            )
        {
            output.push((high * 16 + low) as u8);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn asset_response(request: Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let relative = request.uri().path().trim_start_matches('/');
    if relative.split('/').any(|part| part == "..") {
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Cow::Borrowed(&b"forbidden"[..]))
            .unwrap();
    }
    let root = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = root.join("assets").join(relative);
    let Ok(bytes) = fs::read(&path) else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Cow::Borrowed(&b"not found"[..]))
            .unwrap();
    };
    let content_type = match path.extension().and_then(|value| value.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    Response::builder()
        .header(CONTENT_TYPE, content_type)
        .body(Cow::Owned(bytes))
        .unwrap()
}

fn toolbar_bounds(window: &Window, menu_open: bool) -> Rect {
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);
    Rect {
        position: LogicalPosition::new(0.0, 0.0).into(),
        size: LogicalSize::new(
            size.width,
            if menu_open {
                TOOLBAR_HEIGHT + MENU_OVERLAY_HEIGHT
            } else {
                TOOLBAR_HEIGHT
            },
        )
        .into(),
    }
}

fn content_bounds(window: &Window) -> Rect {
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);
    Rect {
        position: LogicalPosition::new(0.0, TOOLBAR_HEIGHT).into(),
        size: LogicalSize::new(size.width, (size.height - TOOLBAR_HEIGHT).max(0.0)).into(),
    }
}

fn raise_webview(webview: &WebView) {
    let controller = webview.controller();
    let mut hwnd = HWND::default();
    unsafe {
        if controller.ParentWindow(&mut hwnd).is_ok() {
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOP),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

fn make_tab(
    window: &Window,
    proxy: &EventLoopProxy<UserEvent>,
    state: &Rc<RefCell<BrowserState>>,
    id: u64,
    uri: &str,
    incognito: bool,
) -> wry::Result<Tab> {
    let ipc_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    let title_proxy = proxy.clone();
    let download_proxy = proxy.clone();
    let download_state = state.clone();
    let initial_zoom = state.borrow().zoom_for_uri(uri);
    let store_identity_script = crate::store_identity::navigator_override_script();
    let builder = WebViewBuilder::new()
        .with_custom_protocol("ubar".into(), |_, request| asset_response(request))
        .with_url(uri)
        .with_bounds(content_bounds(window))
        .with_visible(false)
        .with_incognito(incognito)
        .with_clipboard(true)
        .with_devtools(true)
        .with_browser_extensions_enabled(true)
        .with_extensions_path(crate::extensions::root())
        .with_initialization_script(content_script())
        .with_initialization_script(store_identity_script)
        .with_ipc_handler(move |request| {
            let _ = ipc_proxy.send_event(UserEvent::Content(id, request.body().clone()));
        })
        .with_on_page_load_handler(move |event, uri| {
            if matches!(event, PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(UserEvent::Loaded(id, uri));
            }
        })
        .with_document_title_changed_handler(move |title| {
            let _ = title_proxy.send_event(UserEvent::Title(id, title));
        })
        .with_download_completed_handler(move |uri, path, success| {
            let Some(path) = path else { return };
            if incognito {
                return;
            }
            let filename = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "download".into());
            let mut state = download_state.borrow_mut();
            let download_id = state.add_download(
                &uri,
                &path.to_string_lossy(),
                &filename,
                Utc::now().timestamp(),
            );
            if let Some(entry) = state.download_mut(download_id) {
                entry.status = if success { "finished" } else { "failed" }.into();
            }
            state.save();
            drop(state);
            let lower = filename.to_ascii_lowercase();
            if success
                && (lower.ends_with(".xpi") || lower.ends_with(".crx"))
                && crate::extensions::install_file(&path).is_ok()
            {
                let _ = download_proxy.send_event(UserEvent::ExtensionsChanged);
            }
        });
    let webview = builder.build_as_child(window)?;
    let _ = webview.zoom(initial_zoom);

    Ok(Tab {
        id,
        webview,
        uri: uri.into(),
        title: "New Tab".into(),
        incognito,
        zoom: initial_zoom,
        lifecycle: TabLifecycle::Active,
        loaded: false,
    })
}

impl App {
    fn request_suspend(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        if tab.lifecycle != TabLifecycle::Active {
            return;
        }
        if !tab.loaded {
            return;
        }
        let Ok(core) = tab.webview.webview().cast::<ICoreWebView2_3>() else {
            return;
        };
        let id = tab.id;
        let proxy = self.proxy.clone();
        let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
            let _ = proxy.send_event(UserEvent::SuspendCompleted(
                id,
                result.is_ok() && suspended,
            ));
            result
        }));
        if unsafe { core.TrySuspend(&handler) }.is_ok() {
            tab.lifecycle = TabLifecycle::SuspendPending;
        }
    }

    fn resume_tab(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        match tab.lifecycle {
            TabLifecycle::Active => {}
            TabLifecycle::SuspendPending => tab.lifecycle = TabLifecycle::Active,
            TabLifecycle::Suspended => {
                if let Ok(core) = tab.webview.webview().cast::<ICoreWebView2_3>()
                    && unsafe { core.Resume() }.is_ok()
                {
                    tab.lifecycle = TabLifecycle::Active;
                }
            }
        }
    }

    fn set_zoom(&mut self, level: f64) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let level = crate::zoom::clamp(level);
        if tab.webview.zoom(level).is_ok() {
            tab.zoom = level;
            if !tab.incognito {
                self.state.borrow_mut().set_zoom_for_uri(&tab.uri, level);
            }
            self.sync_toolbar();
        }
    }

    fn zoom_in(&mut self) {
        let level = self.tabs.get(self.active).map(|tab| tab.zoom).unwrap_or(1.0);
        self.set_zoom(crate::zoom::increase(level));
    }

    fn zoom_out(&mut self) {
        let level = self.tabs.get(self.active).map(|tab| tab.zoom).unwrap_or(1.0);
        self.set_zoom(crate::zoom::decrease(level));
    }

    fn add_tab(&mut self, uri: String) {
        self.add_tab_with_mode(uri, self.private_window);
    }

    fn open_private_window(&self) {
        match std::env::current_exe()
            .map_err(|error| error.to_string())
            .and_then(|executable| {
                Command::new(executable)
                    .arg("--incognito")
                    .spawn()
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
        {
            Ok(()) => {}
            Err(error) => eprintln!("ubar: could not open private window: {error}"),
        }
    }

    fn add_tab_with_mode(&mut self, uri: String, incognito: bool) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        match make_tab(window, &self.proxy, &self.state, id, &uri, incognito) {
            Ok(tab) => {
                if let Some(active) = self.tabs.get(self.active) {
                    let _ = active.webview.set_visible(false);
                }
                if !self.tabs.is_empty() {
                    self.request_suspend(self.active);
                }
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                let _ = self.tabs[self.active].webview.set_visible(true);
                let _ = self.tabs[self.active].webview.focus();
                self.sync_toolbar();
            }
            Err(error) => eprintln!("ubar: could not create WebView2 tab: {error}"),
        }
    }

    fn close_tab(&mut self, id: u64) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let was_active = index == self.active;
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.save_session();
            let _ = self.proxy.send_event(UserEvent::Exit);
        } else {
            if index < self.active {
                self.active -= 1;
            } else if was_active {
                self.active = index.min(self.tabs.len() - 1);
            }
            self.resume_tab(self.active);
            let _ = self.tabs[self.active].webview.set_visible(true);
            let _ = self.tabs[self.active].webview.focus();
            self.sync_toolbar();
        }
    }

    fn select_tab(&mut self, id: u64) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if index == self.active {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active) {
            let _ = tab.webview.set_visible(false);
        }
        self.request_suspend(self.active);
        self.active = index;
        self.resume_tab(index);
        let _ = self.tabs[index].webview.set_visible(true);
        let _ = self.tabs[index].webview.focus();
        self.sync_toolbar();
    }

    fn navigate(&self, input: &str) {
        let engine = self.state.borrow().settings.search_engine.clone();
        if let Some(uri) = normalize_uri(input, &engine)
            && let Some(tab) = self.tabs.get(self.active)
        {
            let _ = tab.webview.load_url(&uri);
        }
    }

    fn open_internal_page(&mut self, uri: &str) {
        self.add_tab(uri.to_string());
    }

    fn toolbar_eval(&self, script: &str) {
        if let Some(toolbar) = &self.toolbar {
            let _ = toolbar.evaluate_script(script);
        }
    }

    fn set_menu_open(&mut self, open: bool) {
        self.menu_open = open;
        if let (Some(window), Some(toolbar)) = (&self.window, &self.toolbar) {
            let _ = toolbar.set_bounds(toolbar_bounds(window, self.menu_open));
            raise_webview(toolbar);
        }
    }

    fn render_internal(&self, id: u64) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == id) else {
            return;
        };
        let state = self.state.borrow();
        let script = if tab.uri.contains("/pages/history/") {
            let items = state
                .history
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "title": item.title,
                        "uri": item.uri,
                        "time": chrono::DateTime::<Utc>::from_timestamp(item.timestamp, 0)
                            .map(|time| time.format("%Y-%m-%d %H:%M").to_string())
                            .unwrap_or_default()
                    })
                })
                .collect::<Vec<_>>();
            format!(
                "window.ubarRenderHistory?.({});",
                serde_json::json!({"items": items})
            )
        } else if tab.uri.contains("/pages/bookmarks/") {
            let items = state
                .bookmarks
                .iter()
                .map(|item| serde_json::json!({"title": item.title, "uri": item.uri}))
                .collect::<Vec<_>>();
            format!(
                "window.ubarRenderBookmarks?.({});",
                serde_json::json!({"items": items})
            )
        } else if tab.uri.contains("/pages/downloads/") {
            let items = state
                .downloads
                .iter()
                .map(|item| {
                    serde_json::json!({
                        "id": item.id,
                        "uri": item.uri,
                        "filename": item.filename,
                        "received": item.received,
                        "total": item.total,
                        "status": if item.status == "finished" { "done" } else { &item.status }
                    })
                })
                .collect::<Vec<_>>();
            format!(
                "window.ubarRenderDownloads?.({});",
                serde_json::json!({"items": items})
            )
        } else if tab.uri.contains("/pages/extensions/") {
            let extensions = crate::extensions::load();
            let items = extensions
                .names
                .iter()
                .zip(extensions.dirs.iter())
                .zip(extensions.pages.iter())
                .map(|((name, dir), page)| {
                    serde_json::json!({
                        "name": name,
                        "dir": dir.to_string_lossy(),
                        "page": page
                    })
                })
                .collect::<Vec<_>>();
            format!(
                "window.ubarRenderExtensions?.({});",
                serde_json::json!({"items": items})
            )
        } else if tab.uri.contains("/pages/settings/") {
            format!(
                "window.ubarRenderSettings?.({});",
                serde_json::json!({
                    "homepageUri": state.settings.homepage_uri,
                    "historyCount": state.history.len(),
                    "bookmarkCount": state.bookmarks.len(),
                    "searchEngine": state.settings.search_engine,
                    "downloadDir": state.settings.download_dir,
                    "theme": state.settings.theme,
                    "defaultZoom": state.settings.default_zoom,
                    "fontSize": state.settings.font_size,
                    "defaults": {},
                    "sites": [],
                    "passwords": []
                })
            )
        } else {
            return;
        };
        let _ = tab.webview.evaluate_script(&script);
    }

    fn handle_internal(&mut self, target: usize, message: &str) {
        let Some(tab) = self.tabs.get(target) else {
            return;
        };
        if !tab.uri.contains("ubar.localhost") && !tab.uri.starts_with("ubar:") {
            return;
        }
        if let Some(uri) = message.strip_prefix("open:") {
            let uri = percent_decode(uri);
            let _ = tab.webview.load_url(&uri);
        } else if message == "clear-history" {
            self.state.borrow_mut().clear_history();
            self.render_internal(tab.id);
        } else if let Some(uri) = message.strip_prefix("delete-bookmark:") {
            self.state
                .borrow_mut()
                .remove_bookmark(&percent_decode(uri));
            self.render_internal(tab.id);
            self.sync_toolbar();
        } else if message == "downloads-clear" {
            let mut state = self.state.borrow_mut();
            state.downloads.retain(|item| item.status == "active");
            state.save();
            drop(state);
            self.render_internal(tab.id);
        } else if let Some(name) = message.strip_prefix("delete-extension:") {
            let extensions = crate::extensions::load();
            if crate::extensions::uninstall(&percent_decode(name), &extensions) {
                self.render_internal(tab.id);
                self.sync_toolbar();
            }
        }
    }

    fn sync_toolbar(&self) {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| serde_json::json!({
                "id": tab.id,
                "title": tab.title,
                "active": tab.id == self.tabs[self.active].id,
                "incognito": tab.incognito
            }))
            .collect::<Vec<_>>();
        let uri = self
            .tabs
            .get(self.active)
            .map(|tab| tab.uri.as_str())
            .unwrap_or("");
        let bookmarked = self.state.borrow().is_bookmarked(uri);
        let zoom = self.tabs.get(self.active).map(|tab| tab.zoom).unwrap_or(1.0);
        let loaded = crate::extensions::load();
        let extensions = loaded
            .names
            .iter()
            .zip(loaded.pages.iter())
            .filter(|(_, page)| !page.is_empty())
            .map(|(name, page)| serde_json::json!({"name": name, "page": page}))
            .collect::<Vec<_>>();
        self.toolbar_eval(&format!(
            "window.ubarRender({}, {}, {}, {}, {});",
            serde_json::to_string(&tabs).unwrap(),
            serde_json::to_string(uri).unwrap(),
            bookmarked,
            zoom,
            serde_json::to_string(&extensions).unwrap()
        ));
    }

    fn handle_command(&mut self, id: Option<u64>, message: &str) {
        let Ok(value) = serde_json::from_str::<Value>(message) else {
            return;
        };
        let command = value["cmd"].as_str().unwrap_or("");
        let target = id
            .and_then(|id| self.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or(self.active);
        match command {
            "navigate" => self.navigate(value["value"].as_str().unwrap_or("")),
            "ready" => self.sync_toolbar(),
            "menu-open" => self.set_menu_open(true),
            "menu-close" => self.set_menu_open(false),
            "open-page" => {
                let uri = match value["value"].as_str().unwrap_or("") {
                    "home" => self.new_tab_uri.clone(),
                    "history" => HISTORY_URI.to_string(),
                    "bookmarks" => BOOKMARKS_URI.to_string(),
                    "downloads" => DOWNLOADS_URI.to_string(),
                    "extensions" => EXTENSIONS_URI.to_string(),
                    "settings" => SETTINGS_URI.to_string(),
                    _ => return,
                };
                self.open_internal_page(&uri);
            }
            "open-extension" => {
                if let Some(uri) = value["value"].as_str() {
                    self.open_internal_page(uri);
                }
            }
            "incognito" => self.open_private_window(),
            "reading-mode" => {
                let _ = self.tabs[target].webview.evaluate_script(
                    r#"(() => {
const id='ubar-reader-style';
const old=document.getElementById(id);
if(old){old.remove();return}
const style=document.createElement('style');style.id=id;
style.textContent='body{max-width:760px!important;margin:auto!important;padding:32px!important;font:18px/1.7 Georgia,serif!important} nav,aside,header,footer,[role=banner],[role=navigation]{display:none!important}';
document.documentElement.append(style);
})()"#,
                );
            }
            "benchmark-ready" => {
                if self.benchmark_restore_started.is_some()
                    && self.benchmark_restore_completed.is_none()
                {
                    self.benchmark_restore_completed = Some(std::time::Instant::now());
                }
            }
            "task-manager" => {
                let mut lines = self
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        format!("{}. {} [{:?}]", index + 1, tab.title, tab.lifecycle)
                    })
                    .collect::<Vec<_>>()
                    .join("\\n");
                let measured = crate::process_metrics::browser_tree_memory();
                let drm = crate::drm::detect().label();
                lines.push_str(&format!(
                    "\\n\\nResident now: {} MiB\\nPrivate committed: {} MiB\\nProcesses: {}\\nMemory target: {} MiB\\nElastic ceiling: {} MiB\\nRenderer limit: {}{}\\nDRM: {}",
                    measured.resident_bytes / (1024 * 1024),
                    measured.committed_bytes / (1024 * 1024),
                    measured.process_count,
                    self.memory_policy.preferred_resident_bytes / (1024 * 1024),
                    self.memory_policy.elastic_ceiling_bytes / (1024 * 1024),
                    self.memory_policy.renderer_limit,
                    if self.memory_policy.constrained { " (constrained mode)" } else { "" },
                    drm
                ));
                self.toolbar_eval(&format!(
                    "alert({})",
                    serde_json::to_string(&lines).unwrap()
                ));
            }
            "internal" => self.handle_internal(target, value["message"].as_str().unwrap_or("")),
            "window-drag" => {
                if let Some(window) = &self.window {
                    let _ = window.drag_window();
                }
            }
            "window-minimize" => {
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            "window-maximize" => {
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            "window-close" => {
                self.save_session();
                let _ = self.proxy.send_event(UserEvent::Exit);
            }
            "back" => {
                let _ = self.tabs[target].webview.evaluate_script("history.back()");
            }
            "forward" => {
                let _ = self.tabs[target]
                    .webview
                    .evaluate_script("history.forward()");
            }
            "reload" => {
                let _ = self.tabs[target].webview.reload();
            }
            "home" => {
                let home = self.state.borrow().settings.homepage_uri.clone();
                let uri = if home.is_empty() {
                    self.new_tab_uri.clone()
                } else {
                    home
                };
                let _ = self.tabs[target].webview.load_url(&uri);
            }
            "new-tab" => self.add_tab(self.new_tab_uri.clone()),
            "close-tab" => {
                let id = value["id"].as_u64().or(id).unwrap_or(self.tabs[target].id);
                self.close_tab(id);
            }
            "select-tab" => {
                if let Some(id) = value["id"].as_u64() {
                    self.select_tab(id);
                }
            }
            "bookmark" => {
                let tab = &self.tabs[target];
                self.state.borrow_mut().toggle_bookmark(
                    &tab.title,
                    &tab.uri,
                    Utc::now().timestamp(),
                );
                self.sync_toolbar();
            }
            "zoom-in" => self.zoom_in(),
            "zoom-out" => self.zoom_out(),
            "zoom-reset" => self.set_zoom(crate::zoom::DEFAULT_ZOOM),
            "devtools" => self.tabs[target].webview.open_devtools(),
            "shortcut" => match value["key"].as_str().unwrap_or("") {
                "l" => self.toolbar_eval("window.ubarFocusAddress()"),
                "t" => self.add_tab(self.new_tab_uri.clone()),
                "w" => self.close_tab(self.tabs[target].id),
                "r" => {
                    let _ = self.tabs[target].webview.reload();
                }
                "d" => {
                    let tab = &self.tabs[target];
                    self.state.borrow_mut().toggle_bookmark(
                        &tab.title,
                        &tab.uri,
                        Utc::now().timestamp(),
                    );
                    self.sync_toolbar();
                }
                "+" | "=" => self.zoom_in(),
                "-" => self.zoom_out(),
                "0" => self.set_zoom(crate::zoom::DEFAULT_ZOOM),
                _ => {}
            },
            _ => {}
        }
    }

    fn save_session(&self) {
        if self.private_window {
            return;
        }
        let mut state = self.state.borrow_mut();
        state.open_tabs = self
            .tabs
            .iter()
            .filter(|tab| !tab.incognito)
            .map(|tab| tab.uri.clone())
            .collect();
        state.save();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = event_loop
            .create_window(
                Window::default_attributes()
                    .with_title(if self.private_window {
                        "uBar Private"
                    } else {
                        "ubar"
                    })
                    .with_decorations(false)
                    .with_inner_size(PhysicalSize::new(1280, 800)),
            )
            .expect("create ubar window");
        let proxy = self.proxy.clone();
        let toolbar = WebViewBuilder::new()
            .with_custom_protocol("ubar".into(), |_, request| asset_response(request))
            .with_browser_extensions_enabled(true)
            .with_extensions_path(crate::extensions::root())
            .with_initialization_script(
                "if(!window.ipc){window.ipc={postMessage:function(m){window.chrome.webview.postMessage(m);}};}",
            )
            .with_transparent(true)
            .with_url(TOOLBAR_URI)
            .with_bounds(toolbar_bounds(&window, self.menu_open))
            .with_ipc_handler(move |request| {
                let _ = proxy.send_event(UserEvent::Toolbar(request.body().clone()));
            })
            .build_as_child(&window)
            .expect("create ubar toolbar");
        self.window = Some(window);
        self.toolbar = Some(toolbar);
        if let Some(toolbar) = &self.toolbar {
            raise_webview(toolbar);
        }

        let saved = self.state.borrow().open_tabs.clone();
        let initial = if self.private_window {
            vec![self.new_tab_uri.clone()]
        } else if self.benchmark_started.is_some() {
            self.benchmark_urls.first().cloned().into_iter().collect()
        } else if saved.is_empty() {
            vec![self.new_tab_uri.clone()]
        } else {
            saved
        };
        for uri in initial {
            self.add_tab(uri);
        }

    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.benchmark_reported {
            return;
        }
        let Some(started) = self.benchmark_started else {
            return;
        };
        let now = std::time::Instant::now();
        if self.benchmark_trimmed_at.is_none()
            && self
                .benchmark_settled_at
                .is_some_and(|loaded| now.duration_since(loaded).as_secs() >= 10)
        {
            let trimmed = crate::process_metrics::trim_browser_children_working_sets();
            eprintln!("ubar benchmark: trimmed {trimmed} child working sets");
            self.benchmark_trimmed_at = Some(now);
        }
        if self.benchmark_restore_started.is_none()
            && self
                .benchmark_trimmed_at
                .is_some_and(|trimmed| now.duration_since(trimmed).as_secs() >= 2)
            && let Some(id) = self.tabs.first().map(|tab| tab.id)
        {
            self.benchmark_restore_started = Some(now);
            self.select_tab(id);
            if let Some(tab) = self.tabs.get(self.active) {
                let _ = tab.webview.evaluate_script(
                    "requestAnimationFrame(()=>requestAnimationFrame(()=>window.ipc.postMessage(JSON.stringify({cmd:'benchmark-ready'}))))",
                );
            }
        }
        let settled = self
            .benchmark_restore_completed
            .is_some_and(|restored| now.duration_since(restored).as_secs() >= 3);
        let timed_out = now.duration_since(started).as_secs() >= 90;
        if settled || timed_out {
            self.benchmark_reported = true;
            let memory = crate::process_metrics::browser_tree_memory();
            let report = serde_json::json!({
                "profile": "five-modern-tabs",
                "complete": settled,
                "elapsed_seconds": now.duration_since(started).as_secs(),
                "loaded_tabs": self.benchmark_loaded.len(),
                "tab_count": self.tabs.len(),
                "resident_bytes": memory.resident_bytes,
                "resident_mib": memory.resident_bytes / (1024 * 1024),
                "private_committed_bytes": memory.committed_bytes,
                "private_committed_mib": memory.committed_bytes / (1024 * 1024),
                "restore_animation_frame_ms": self.benchmark_restore_started.zip(self.benchmark_restore_completed).map(|(start, end)| end.duration_since(start).as_millis()),
                "process_count": memory.process_count,
                "physical_memory_bytes": self.memory_policy.physical_bytes,
                "preferred_resident_bytes": self.memory_policy.preferred_resident_bytes,
                "elastic_ceiling_bytes": self.memory_policy.elastic_ceiling_bytes,
                "tabs": self.tabs.iter().map(|tab| serde_json::json!({
                    "uri": tab.uri,
                    "title": tab.title,
                    "lifecycle": format!("{:?}", tab.lifecycle),
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            event_loop.exit();
        } else {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                now + std::time::Duration::from_secs(1),
            ));
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_session();
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(window) = &self.window {
                    if let Some(toolbar) = &self.toolbar {
                        let _ = toolbar.set_bounds(toolbar_bounds(window, self.menu_open));
                        raise_webview(toolbar);
                    }
                    for tab in &self.tabs {
                        let _ = tab.webview.set_bounds(content_bounds(window));
                    }
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Exit => event_loop.exit(),
            UserEvent::Toolbar(message) => self.handle_command(None, &message),
            UserEvent::Content(id, message) => self.handle_command(Some(id), &message),
            UserEvent::Loaded(id, uri) => {
                if uri == "about:blank"
                    && self
                        .tabs
                        .iter()
                        .find(|tab| tab.id == id)
                        .is_some_and(|tab| tab.uri != "about:blank")
                {
                    return;
                }
                if self.benchmark_started.is_some() {
                    self.benchmark_loaded.insert(id);
                    if self.benchmark_loaded.len() == self.benchmark_urls.len()
                        && self.benchmark_settled_at.is_none()
                    {
                        self.benchmark_settled_at = Some(std::time::Instant::now());
                    }
                }
                let zoom = self.state.borrow().zoom_for_uri(&uri);
                let mut loaded_index = None;
                if let Some((index, tab)) = self
                    .tabs
                    .iter_mut()
                    .enumerate()
                    .find(|(_, tab)| tab.id == id)
                {
                    tab.uri = uri.clone();
                    tab.zoom = zoom;
                    tab.loaded = true;
                    loaded_index = Some(index);
                    let _ = tab.webview.zoom(zoom);
                    if !tab.incognito
                        && !uri.contains("ubar.localhost")
                        && !uri.starts_with("ubar:")
                    {
                        self.state.borrow_mut().add_history(
                            &tab.title,
                            &uri,
                            Utc::now().timestamp(),
                        );
                    }
                }
                if let Some(index) = loaded_index
                    && index != self.active
                {
                    self.request_suspend(index);
                }
                self.render_internal(id);
                self.sync_toolbar();
                if self.benchmark_started.is_some()
                    && self.benchmark_next < self.benchmark_urls.len()
                {
                    let next = self.benchmark_urls[self.benchmark_next].clone();
                    self.benchmark_next += 1;
                    self.add_tab(next);
                }
            }
            UserEvent::Title(id, title) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.title = title;
                }
                if self.tabs.get(self.active).is_some_and(|tab| tab.id == id)
                    && let Some(window) = &self.window
                {
                    window.set_title(&format!(
                        "{} - {}",
                        self.tabs[self.active].title,
                        if self.private_window {
                            "uBar Private"
                        } else {
                            "ubar"
                        }
                    ));
                }
                self.sync_toolbar();
            }
            UserEvent::SuspendCompleted(id, suspended) => {
                if let Some(index) = self.tabs.iter().position(|tab| tab.id == id) {
                    self.tabs[index].lifecycle = if suspended {
                        TabLifecycle::Suspended
                    } else {
                        TabLifecycle::Active
                    };
                    if index == self.active {
                        self.resume_tab(index);
                    }
                }
            }
            UserEvent::ExtensionsChanged => {
                self.sync_toolbar();
                let ids = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.uri.contains("/pages/extensions/"))
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                for id in ids {
                    self.render_internal(id);
                }
            }
        }
    }
}

pub fn run() {
    let benchmark = std::env::args().any(|argument| argument == "--benchmark-five-tabs");
    let private_window = std::env::args().any(|argument| argument == "--incognito");
    let benchmark_urls = benchmark.then(benchmark_urls).unwrap_or_default();
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        window: None,
        toolbar: None,
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        menu_open: false,
        state: Rc::new(RefCell::new(BrowserState::load())),
        new_tab_uri: NEW_TAB_URI.into(),
        memory_policy: crate::memory_policy::MemoryPolicy::detect(),
        private_window,
        benchmark_started: benchmark.then(std::time::Instant::now),
        benchmark_next: usize::from(benchmark),
        benchmark_urls,
        benchmark_loaded: HashSet::new(),
        benchmark_settled_at: None,
        benchmark_trimmed_at: None,
        benchmark_restore_started: None,
        benchmark_restore_completed: None,
        benchmark_reported: false,
    };
    event_loop.run_app(&mut app).expect("run ubar");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses_and_searches() {
        assert_eq!(
            normalize_uri("example.com", "duckduckgo").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_uri("two words", "google").unwrap(),
            "https://www.google.com/search?q=two+words"
        );
        assert_eq!(
            percent_decode("https%3A%2F%2Fexample.com%2Fa%20b"),
            "https://example.com/a b"
        );
    }
}
