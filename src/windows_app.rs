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
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder};

const TOOLBAR_HEIGHT: f64 = 82.0;
const NEW_TAB_URI: &str = "ubar://localhost/newtab/index.html";
const TOOLBAR_URI: &str = "ubar://localhost/windows/index.html";

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
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Window>,
    toolbar: Option<WebView>,
    tabs: Vec<Tab>,
    active: usize,
    next_id: u64,
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
"#
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

fn toolbar_bounds(window: &Window) -> Rect {
    let scale = window.scale_factor();
    let size = window.inner_size();
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(size.width, (TOOLBAR_HEIGHT * scale) as u32).into(),
    }
}

fn content_bounds(window: &Window) -> Rect {
    let scale = window.scale_factor();
    let size = window.inner_size();
    let top = (TOOLBAR_HEIGHT * scale) as u32;
    Rect {
        position: PhysicalPosition::new(0, top as i32).into(),
        size: PhysicalSize::new(size.width, size.height.saturating_sub(top)).into(),
    }
}

fn make_tab(
    window: &Window,
    proxy: &EventLoopProxy<UserEvent>,
    state: &Rc<RefCell<BrowserState>>,
    id: u64,
    uri: &str,
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
    })
}

impl App {
    fn add_tab(&mut self, uri: String) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        let id = self.next_id;
        self.next_id += 1;
        match make_tab(window, &self.proxy, &self.state, id, &uri) {
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

    fn toolbar_eval(&self, script: &str) {
        if let Some(toolbar) = &self.toolbar {
            let _ = toolbar.evaluate_script(script);
        }
    }

    fn sync_toolbar(&self) {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| serde_json::json!({"id":tab.id,"title":tab.title,"active":tab.id == self.tabs[self.active].id}))
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
            .with_url(TOOLBAR_URI)
            .with_bounds(toolbar_bounds(&window))
            .with_ipc_handler(move |request| {
                let _ = proxy.send_event(UserEvent::Toolbar(request.body().clone()));
            })
            .build_as_child(&window)
            .expect("create ubar toolbar");
        self.window = Some(window);
        self.toolbar = Some(toolbar);

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
                        let _ = toolbar.set_bounds(toolbar_bounds(window));
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
    }
}
