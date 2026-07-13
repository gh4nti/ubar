use crate::state::BrowserState;
use chrono::Utc;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::rc::Rc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};
use windows::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos},
};
use wry::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewExtWindows};

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
    Exit,
}

struct Tab {
    id: u64,
    webview: WebView,
    uri: String,
    title: String,
    incognito: bool,
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
  if (['l','t','w','r','d'].includes(key)) {
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
    let download_state = state.clone();
    let webview = WebViewBuilder::new()
        .with_custom_protocol("ubar".into(), |_, request| asset_response(request))
        .with_url(uri)
        .with_bounds(content_bounds(window))
        .with_visible(false)
        .with_incognito(incognito)
        .with_clipboard(true)
        .with_devtools(true)
        .with_initialization_script(content_script())
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
        })
        .build_as_child(window)?;

    Ok(Tab {
        id,
        webview,
        uri: uri.into(),
        title: "New Tab".into(),
        incognito,
    })
}

impl App {
    fn add_tab(&mut self, uri: String) {
        self.add_tab_with_mode(uri, false);
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
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.save_session();
            let _ = self.proxy.send_event(UserEvent::Exit);
        } else {
            self.active = self.active.min(self.tabs.len() - 1);
            let _ = self.tabs[self.active].webview.set_visible(true);
            let _ = self.tabs[self.active].webview.focus();
            self.sync_toolbar();
        }
    }

    fn select_tab(&mut self, id: u64) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        if let Some(tab) = self.tabs.get(self.active) {
            let _ = tab.webview.set_visible(false);
        }
        self.active = index;
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
            "window.ubarRenderExtensions?.({items:[]});".into()
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
        self.toolbar_eval(&format!(
            "window.ubarRender({}, {}, {});",
            serde_json::to_string(&tabs).unwrap(),
            serde_json::to_string(uri).unwrap(),
            bookmarked
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
            "incognito" => self.add_tab_with_mode(self.new_tab_uri.clone(), true),
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
            "task-manager" => {
                let lines = self
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| format!("{}. {}", index + 1, tab.title))
                    .collect::<Vec<_>>()
                    .join("\\n");
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
                _ => {}
            },
            _ => {}
        }
    }

    fn save_session(&self) {
        let mut state = self.state.borrow_mut();
        state.open_tabs = self.tabs.iter().map(|tab| tab.uri.clone()).collect();
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
                    .with_title("ubar")
                    .with_decorations(false)
                    .with_inner_size(PhysicalSize::new(1280, 800)),
            )
            .expect("create ubar window");
        let proxy = self.proxy.clone();
        let toolbar = WebViewBuilder::new()
            .with_custom_protocol("ubar".into(), |_, request| asset_response(request))
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
        let initial = if saved.is_empty() {
            vec![self.new_tab_uri.clone()]
        } else {
            saved
        };
        for uri in initial {
            self.add_tab(uri);
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
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.uri = uri.clone();
                    if !uri.contains("ubar.localhost") && !uri.starts_with("ubar:") {
                        self.state.borrow_mut().add_history(
                            &tab.title,
                            &uri,
                            Utc::now().timestamp(),
                        );
                    }
                }
                self.render_internal(id);
                self.sync_toolbar();
            }
            UserEvent::Title(id, title) => {
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
                    tab.title = title;
                }
                if self.tabs.get(self.active).is_some_and(|tab| tab.id == id)
                    && let Some(window) = &self.window
                {
                    window.set_title(&format!("{} - ubar", self.tabs[self.active].title));
                }
                self.sync_toolbar();
            }
        }
    }
}

pub fn run() {
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
