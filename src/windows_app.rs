use crate::state::BrowserState;
use chrono::Utc;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED, COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
    ICoreWebView2_3, ICoreWebView2_4, ICoreWebView2DownloadOperation, ICoreWebView2Settings2,
};
use webview2_com::{
    BytesReceivedChangedEventHandler, DownloadStartingEventHandler, StateChangedEventHandler,
    TrySuspendCompletedHandler, take_pwstr,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{ResizeDirection, Window, WindowId};
use windows::core::{HSTRING, Interface, PCWSTR, PWSTR, w};
use windows::Win32::{
    Foundation::HWND,
    UI::{
        Shell::ShellExecuteW,
        WindowsAndMessaging::{
            HWND_TOP, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos,
        },
    },
};
use wry::dpi::{LogicalPosition, LogicalSize, PhysicalSize};
use wry::http::{Request, Response, StatusCode, header::CONTENT_TYPE};
use wry::{
    PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows,
};

const TOOLBAR_HEIGHT: f64 = 82.0;
const MENU_OVERLAY_HEIGHT: f64 = 360.0;
const NEW_TAB_URI: &str = "ubar://localhost/newtab/index.html";
const NEW_TAB_WEBVIEW2_URI: &str = "http://ubar.localhost/newtab/index.html";
const TOOLBAR_URI: &str = "ubar://localhost/windows/index.html";
const HISTORY_URI: &str = "ubar://localhost/pages/history/index.html";
const BOOKMARKS_URI: &str = "ubar://localhost/pages/bookmarks/index.html";
const DOWNLOADS_URI: &str = "ubar://localhost/pages/downloads/index.html";
const EXTENSIONS_URI: &str = "ubar://localhost/pages/extensions/index.html";
const SETTINGS_URI: &str = "ubar://localhost/pages/settings/index.html";
static DEFAULT_WEBVIEW_USER_AGENT: OnceLock<String> = OnceLock::new();

#[derive(Debug)]
enum UserEvent {
    Toolbar(String),
    Content(u64, String),
    Loaded(u64, String),
    Title(u64, String),
    SuspendCompleted(u64, bool),
    DownloadsChanged,
    DownloadProgress(u64, u64, u64),
    DownloadEnded(u64, bool),
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
    download_operations: Rc<RefCell<BTreeMap<u64, ICoreWebView2DownloadOperation>>>,
    cancel_requested: HashSet<u64>,
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
  if (['l','t','w','r','d','j','+','=','-','0','tab','1','2','3','4','5','6','7','8','9'].includes(key)
      || (!event.shiftKey && ['h',','].includes(key))
      || (event.shiftKey && ['o','x','n'].includes(key))) {
    event.preventDefault();
    window.ipc.postMessage(JSON.stringify({cmd:'shortcut',key,shift:event.shiftKey}));
  }
}, true);
{
const ubarResizeDirection = (event, top, bottom) => {
  const edge = 6;
  const left = event.clientX < edge;
  const right = event.clientX >= innerWidth - edge;
  const north = top && event.clientY < edge;
  const south = bottom && event.clientY >= innerHeight - edge;
  return north ? (left ? 'nw' : right ? 'ne' : 'n')
    : south ? (left ? 'sw' : right ? 'se' : 's')
    : left ? 'w' : right ? 'e' : '';
};
const ubarResizeCursor = {n:'ns-resize',s:'ns-resize',e:'ew-resize',w:'ew-resize',nw:'nwse-resize',se:'nwse-resize',ne:'nesw-resize',sw:'nesw-resize'};
window.addEventListener('pointermove', event => {
  const direction = ubarResizeDirection(event, false, true);
  if (direction) {
    if (!document.getElementById('ubar-resize-cursor')) {
      const style = document.createElement('style');
      style.id = 'ubar-resize-cursor';
      style.textContent = 'html[data-ubar-resize] *{cursor:var(--ubar-resize-cursor)!important}';
      document.head.append(style);
    }
    document.documentElement.dataset.ubarResize = direction;
    document.documentElement.style.setProperty('--ubar-resize-cursor', ubarResizeCursor[direction]);
  } else if (document.documentElement.dataset.ubarResize) {
    delete document.documentElement.dataset.ubarResize;
    document.documentElement.style.removeProperty('--ubar-resize-cursor');
  }
}, true);
window.addEventListener('pointerdown', event => {
  if (event.button !== 0) return;
  const direction = ubarResizeDirection(event, false, true);
  if (!direction) return;
  event.preventDefault();
  event.stopImmediatePropagation();
  window.ipc.postMessage(JSON.stringify({cmd:'window-resize',value:direction}));
}, true);
}
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
            (if menu_open {
                TOOLBAR_HEIGHT + MENU_OVERLAY_HEIGHT
            } else {
                TOOLBAR_HEIGHT
            })
            .min(size.height.max(0.0)),
        )
        .into(),
    }
}

fn content_bounds(window: &Window) -> Rect {
    let scale = window.scale_factor();
    let size = window.inner_size().to_logical::<f64>(scale);
    Rect {
        position: LogicalPosition::new(0.0, TOOLBAR_HEIGHT).into(),
        size: LogicalSize::new(
            size.width,
            (size.height - TOOLBAR_HEIGHT).max(0.0),
        )
        .into(),
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

fn download_directory(state: &BrowserState) -> PathBuf {
    if state.settings.download_dir.is_empty() {
        dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    } else {
        PathBuf::from(&state.settings.download_dir)
    }
}

fn is_empty_new_tab(uri: &str) -> bool {
    uri == NEW_TAB_URI || uri == NEW_TAB_WEBVIEW2_URI
}

fn remember_default_user_agent(webview: &WebView) {
    if DEFAULT_WEBVIEW_USER_AGENT.get().is_some() {
        return;
    }
    let Ok(settings) = (unsafe { webview.webview().Settings() }) else { return };
    let Ok(settings) = settings.cast::<ICoreWebView2Settings2>() else { return };
    let mut value = PWSTR::null();
    if unsafe { settings.UserAgent(&mut value) }.is_ok() {
        let value = take_pwstr(value);
        if !value.is_empty() {
            let _ = DEFAULT_WEBVIEW_USER_AGENT.set(value);
        }
    }
}

fn apply_site_user_agent(webview: &WebView, uri: &str) {
    let user_agent = crate::store_identity::for_uri(uri)
        .map(crate::store_identity::user_agent)
        .or_else(|| DEFAULT_WEBVIEW_USER_AGENT.get().map(String::as_str));
    let Some(user_agent) = user_agent else { return };
    let Ok(settings) = (unsafe { webview.webview().Settings() }) else { return };
    let Ok(settings) = settings.cast::<ICoreWebView2Settings2>() else { return };
    let _ = unsafe { settings.SetUserAgent(&HSTRING::from(user_agent)) };
}

fn make_tab(
    window: &Window,
    proxy: &EventLoopProxy<UserEvent>,
    state: &Rc<RefCell<BrowserState>>,
    id: u64,
    uri: &str,
    incognito: bool,
    download_operations: &Rc<RefCell<BTreeMap<u64, ICoreWebView2DownloadOperation>>>,
) -> wry::Result<Tab> {
    let ipc_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    let title_proxy = proxy.clone();
    let download_proxy = proxy.clone();
    let download_start_proxy = proxy.clone();
    let download_state = state.clone();
    let download_start_state = state.clone();
    let native_tracking = Rc::new(Cell::new(false));
    let completion_has_native_tracking = native_tracking.clone();
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
        .with_initialization_script(crate::store_identity::install_helper_script())
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
        .with_download_started_handler(move |uri, path| {
            let mut directory = download_directory(&download_start_state.borrow());
            if fs::create_dir_all(&directory).is_err() {
                return false;
            }
            let Ok(absolute) = dunce::canonicalize(&directory) else {
                return false;
            };
            directory = absolute;
            let store_crx = uri.contains("clients2.google.com/service/update2/crx");
            let filename = if store_crx {
                std::ffi::OsStr::new("extension.crx")
            } else {
                path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("download"))
            };
            let mut destination = directory.join(filename);
            let stem = destination.file_stem().unwrap_or_default().to_os_string();
            let extension = destination.extension().map(|value| value.to_os_string());
            for suffix in 1.. {
                if !destination.exists() {
                    *path = destination;
                    if !incognito {
                        let filename = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "download".into());
                        let mut state = download_start_state.borrow_mut();
                        state.add_download(
                            &uri,
                            &path.to_string_lossy(),
                            &filename,
                            Utc::now().timestamp(),
                        );
                        state.save();
                        drop(state);
                        let _ = download_start_proxy.send_event(UserEvent::DownloadsChanged);
                    }
                    return true;
                }
                let mut name = stem.clone();
                name.push(format!(" ({suffix})"));
                destination = directory.join(name);
                if let Some(extension) = &extension {
                    destination.set_extension(extension);
                }
            }
            false
        })
        .with_download_completed_handler(move |uri, path, success| {
            if incognito {
                return;
            }
            let filename = path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "download".into());
            let mut state = download_state.borrow_mut();
            let existing = state
                .downloads
                .iter()
                .find(|entry| {
                    path.as_ref()
                        .is_some_and(|path| entry.destination == path.to_string_lossy())
                })
                .map(|entry| entry.id)
                .or_else(|| {
                    let mut matches = state
                        .downloads
                        .iter()
                        .filter(|entry| entry.status == "active" && entry.uri == uri);
                    let first = matches.next()?.id;
                    matches.next().is_none().then_some(first)
                });
            if existing.is_none() && path.is_none() && !completion_has_native_tracking.get() {
                // Old runtimes lack exact native operation IDs. Fail all ambiguous matches
                // conservatively instead of inventing a duplicate or leaving them active.
                let mut changed = false;
                for entry in state
                    .downloads
                    .iter_mut()
                    .filter(|entry| entry.status == "active" && entry.uri == uri)
                {
                    entry.status = "failed".into();
                    changed = true;
                }
                if changed {
                    state.save();
                    drop(state);
                    let _ = download_proxy.send_event(UserEvent::DownloadsChanged);
                }
                return;
            }
            // A pathless callback cannot identify one of several same-URI downloads.
            // The native operation state callback below owns that exact-ID resolution.
            let Some(download_id) = existing else { return };
            if let Some(entry) = state.download_mut(download_id) {
                if entry.status != "cancelled" {
                    entry.status = if success { "done" } else { "failed" }.into();
                }
            }
            state.save();
            drop(state);
            let _ = download_proxy.send_event(UserEvent::DownloadsChanged);
            let lower = filename.to_ascii_lowercase();
            if success
                && (lower.ends_with(".xpi") || lower.ends_with(".crx"))
                && path.as_ref().is_some_and(|path| crate::extensions::install_file(path).is_ok())
            {
                let _ = download_proxy.send_event(UserEvent::ExtensionsChanged);
            }
        });
    let webview = builder.build_as_child(window)?;
    if !incognito {
        let Ok(native) = webview.webview().cast::<ICoreWebView2_4>() else {
            let _ = webview.zoom(initial_zoom);
            return Ok(Tab {
                id,
                webview,
                uri: uri.into(),
                title: "New Tab".into(),
                incognito,
                zoom: initial_zoom,
                lifecycle: TabLifecycle::Active,
                loaded: false,
            });
        };
        let tracked = download_operations.clone();
        let observed_state = state.clone();
        let observed_proxy = proxy.clone();
        let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let operation = unsafe { args.DownloadOperation()? };
            let mut path = PWSTR::null();
            let path = if unsafe { args.ResultFilePath(&mut path) }.is_ok() {
                take_pwstr(path)
            } else {
                String::new()
            };
            let state = observed_state.borrow();
            let id = state.downloads.iter()
                .find(|entry| entry.status == "active" && entry.destination == path)
                .map(|entry| entry.id);
            drop(state);
            let Some(id) = id else { return Ok(()) };
            tracked.borrow_mut().insert(id, operation.clone());

            let progress_proxy = observed_proxy.clone();
            let mut progress_token = 0;
            unsafe { operation.add_BytesReceivedChanged(
                &BytesReceivedChangedEventHandler::create(Box::new(move |operation, _| {
                    let Some(operation) = operation else { return Ok(()) };
                    let mut received = 0i64;
                    let mut total = 0i64;
                    operation.BytesReceived(&mut received)?;
                    operation.TotalBytesToReceive(&mut total)?;
                    let _ = progress_proxy.send_event(UserEvent::DownloadProgress(
                        id, received.max(0) as u64, total.max(0) as u64,
                    ));
                    Ok(())
                })),
                &mut progress_token,
            )?; }

            let mut received = 0i64;
            let mut total = 0i64;
            unsafe {
                operation.BytesReceived(&mut received)?;
                operation.TotalBytesToReceive(&mut total)?;
            }
            let _ = observed_proxy.send_event(UserEvent::DownloadProgress(
                id, received.max(0) as u64, total.max(0) as u64,
            ));

            let end_proxy = observed_proxy.clone();
            let mut state_token = 0;
            unsafe { operation.add_StateChanged(
                &StateChangedEventHandler::create(Box::new(move |operation, _| {
                    let Some(operation) = operation else { return Ok(()) };
                    let mut state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
                    operation.State(&mut state)?;
                    if state != COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
                        let _ = end_proxy.send_event(UserEvent::DownloadEnded(
                            id,
                            state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
                        ));
                    }
                    Ok(())
                })),
                &mut state_token,
            )?; }
            Ok(())
        }));
        let mut token = 0;
        native_tracking.set(unsafe { native.add_DownloadStarting(&handler, &mut token) }.is_ok());
    }
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
    fn cancel_download(&mut self, id: u64) {
        let operation = self.download_operations.borrow().get(&id).cloned();
        if operation.is_some_and(|operation| unsafe { operation.Cancel() }.is_ok()) {
            self.cancel_requested.insert(id);
        }
    }

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
        match make_tab(window, &self.proxy, &self.state, id, &uri, incognito,
                       &self.download_operations) {
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
                self.sync_toolbar();
                self.focus_active_tab();
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
            if let Some(window) = &self.window {
                let _ = self.tabs[self.active].webview.set_bounds(content_bounds(window));
            }
            let _ = self.tabs[self.active].webview.set_visible(true);
            self.sync_toolbar();
            self.focus_active_tab();
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
        if let Some(window) = &self.window {
            let _ = self.tabs[index].webview.set_bounds(content_bounds(window));
        }
        let _ = self.tabs[index].webview.set_visible(true);
        self.sync_toolbar();
        self.focus_active_tab();
    }

    fn move_tab(&mut self, id: u64, target_id: u64, after: bool) {
        let Some(source) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let Some(target) = self.tabs.iter().position(|tab| tab.id == target_id) else {
            return;
        };
        if source == target {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(source);
        let insertion = target + usize::from(after);
        let destination = (insertion - usize::from(source < insertion)).min(self.tabs.len());
        self.tabs.insert(destination, tab);
        self.active = self.tabs.iter().position(|tab| tab.id == active_id).unwrap_or(0);
        self.sync_toolbar();
        self.save_session();
    }

    fn switch_tab_shortcut(&mut self, key: &str, shift: bool) {
        if self.tabs.is_empty() {
            return;
        }
        let index = if key == "tab" {
            if shift {
                self.active.checked_sub(1).unwrap_or(self.tabs.len() - 1)
            } else {
                (self.active + 1) % self.tabs.len()
            }
        } else if key == "9" {
            self.tabs.len() - 1
        } else {
            let Some(index) = key.as_bytes().first()
                .and_then(|key| key.checked_sub(b'1')).map(usize::from) else { return };
            if index >= self.tabs.len() { return; }
            index
        };
        self.select_tab(self.tabs[index].id);
    }

    fn navigate(&self, input: &str) {
        let engine = self.state.borrow().settings.search_engine.clone();
        if let Some(uri) = normalize_uri(input, &engine)
            && let Some(tab) = self.tabs.get(self.active)
        {
            apply_site_user_agent(&tab.webview, &uri);
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

    fn focus_address_bar(&self) {
        if let Some(toolbar) = &self.toolbar {
            let _ = toolbar.focus();
            let _ = toolbar.evaluate_script("setTimeout(() => window.ubarFocusAddress?.(), 0)");
        }
    }

    fn focus_active_tab(&self) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        if is_empty_new_tab(&tab.uri) {
            self.focus_address_bar();
        } else {
            let _ = tab.webview.focus();
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
            apply_site_user_agent(&tab.webview, &uri);
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
        } else if message == "downloads-folder" {
            let directory = download_directory(&self.state.borrow());
            if fs::create_dir_all(&directory).is_ok() {
                let _ = Command::new("explorer.exe").arg(directory).spawn();
            }
        } else if let Some(id) = message
            .strip_prefix("download-cancel:")
            .and_then(|id| id.parse().ok())
        {
            self.cancel_download(id);
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
        let state = self.state.borrow();
        let downloads = state.downloads.iter().take(6).map(|entry| serde_json::json!({
                "id": entry.id,
                "filename": entry.filename,
                "status": entry.status,
                "received": entry.received,
                "total": entry.total,
                "destination": entry.destination
            })).collect::<Vec<_>>();
        drop(state);
        self.toolbar_eval(&format!("window.ubarRenderDownloads({});",
            serde_json::to_string(&downloads).unwrap()));
    }

    fn handle_command(&mut self, id: Option<u64>, message: &str) {
        let Ok(value) = serde_json::from_str::<Value>(message) else {
            return;
        };
        let command = value["cmd"].as_str().unwrap_or("");
        let toolbar_request = id.is_none();
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
            "download-cancel" if toolbar_request => {
                if let Some(id) = value["id"].as_u64() {
                    self.cancel_download(id);
                }
            }
            "download-open" | "download-show" if toolbar_request => {
                let Some(id) = value["id"].as_u64() else { return };
                let destination = self.state.borrow().downloads.iter()
                    .find(|entry| entry.id == id && entry.status == "done")
                    .map(|entry| entry.destination.clone());
                let Some(destination) = destination else { return };
                let path = PathBuf::from(destination);
                if !path.is_file() { return; }
                let mut command = Command::new("explorer.exe");
                if value["cmd"] == "download-show" {
                    command.arg(format!("/select,{}", path.display()));
                    let _ = command.spawn();
                } else {
                    let wide = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
                    unsafe {
                        ShellExecuteW(
                            None,
                            w!("open"),
                            PCWSTR(wide.as_ptr()),
                            PCWSTR::null(),
                            PCWSTR::null(),
                            SW_SHOWNORMAL,
                        );
                    }
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
            "window-resize" => {
                let direction = match value["value"].as_str() {
                    Some("n") => Some(ResizeDirection::North),
                    Some("s") => Some(ResizeDirection::South),
                    Some("e") => Some(ResizeDirection::East),
                    Some("w") => Some(ResizeDirection::West),
                    Some("ne") => Some(ResizeDirection::NorthEast),
                    Some("nw") => Some(ResizeDirection::NorthWest),
                    Some("se") => Some(ResizeDirection::SouthEast),
                    Some("sw") => Some(ResizeDirection::SouthWest),
                    _ => None,
                };
                if let (Some(window), Some(direction)) = (&self.window, direction)
                    && !window.is_maximized()
                {
                    let _ = window.drag_resize_window(direction);
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
            "move-tab" => {
                if let (Some(id), Some(target_id)) =
                    (value["id"].as_u64(), value["target"].as_u64())
                {
                    self.move_tab(id, target_id, value["after"].as_bool().unwrap_or(false));
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
                key @ ("tab" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => {
                    self.switch_tab_shortcut(key, value["shift"].as_bool().unwrap_or(false));
                }
                "l" => self.focus_address_bar(),
                "t" => self.add_tab(self.new_tab_uri.clone()),
                "n" if value["shift"].as_bool().unwrap_or(false) => self.open_private_window(),
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
                "j" => self.toolbar_eval("window.ubarShowDownloads?.()"),
                "h" if !value["shift"].as_bool().unwrap_or(false) => {
                    self.open_internal_page(HISTORY_URI);
                }
                "," if !value["shift"].as_bool().unwrap_or(false) => {
                    self.open_internal_page(SETTINGS_URI);
                }
                "o" if value["shift"].as_bool().unwrap_or(false) => {
                    self.open_internal_page(BOOKMARKS_URI);
                }
                "x" if value["shift"].as_bool().unwrap_or(false) => {
                    self.open_internal_page(EXTENSIONS_URI);
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
        remember_default_user_agent(&toolbar);
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
                    if let Some(tab) = self.tabs.get(self.active) {
                        let _ = tab.webview.set_bounds(content_bounds(window));
                    }
                    if let Some(toolbar) = &self.toolbar {
                        let _ = toolbar.set_bounds(toolbar_bounds(window, self.menu_open));
                        if self.menu_open {
                            raise_webview(toolbar);
                        }
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
            UserEvent::DownloadsChanged => {
                let pages = self
                    .tabs
                    .iter()
                    .filter(|tab| tab.uri.contains("/pages/downloads/"))
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                for id in pages {
                    self.render_internal(id);
                }
                self.sync_toolbar();
            }
            UserEvent::DownloadProgress(id, received, total) => {
                if let Some(entry) = self.state.borrow_mut().download_mut(id)
                    && entry.status == "active"
                {
                    entry.received = entry.received.max(received);
                    if total >= entry.received {
                        entry.total = total;
                    }
                }
                let pages = self.tabs.iter()
                    .filter(|tab| tab.uri.contains("/pages/downloads/"))
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                for id in pages { self.render_internal(id); }
                self.sync_toolbar();
            }
            UserEvent::DownloadEnded(id, completed) => {
                self.download_operations.borrow_mut().remove(&id);
                let cancelled = self.cancel_requested.remove(&id);
                let mut state = self.state.borrow_mut();
                if let Some(entry) = state.download_mut(id) {
                    entry.status = if cancelled {
                        "cancelled"
                    } else if completed {
                        "done"
                    } else {
                        "failed"
                    }.into();
                }
                state.save();
                drop(state);
                let pages = self.tabs.iter()
                    .filter(|tab| tab.uri.contains("/pages/downloads/"))
                    .map(|tab| tab.id)
                    .collect::<Vec<_>>();
                for id in pages { self.render_internal(id); }
                self.sync_toolbar();
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
    let mut browser_state = BrowserState::load();
    let mut recovered_download = false;
    for download in &mut browser_state.downloads {
        if download.status == "active" {
            download.status = "interrupted".into();
            recovered_download = true;
        }
    }
    if recovered_download {
        browser_state.save();
    }
    let mut app = App {
        proxy,
        window: None,
        toolbar: None,
        tabs: Vec::new(),
        active: 0,
        next_id: 1,
        menu_open: false,
        state: Rc::new(RefCell::new(browser_state)),
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
        download_operations: Rc::new(RefCell::new(BTreeMap::new())),
        cancel_requested: HashSet::new(),
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
