use crate::passwords::Vault;
use crate::state::{asset_uri, BrowserState, SettingsData};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chrono::Utc;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, DragSource, DropTarget, Entry,
    Align,
    EventControllerKey, EventControllerScroll, EventControllerScrollFlags, GestureClick, Image,
    HeaderBar, Label, MenuButton, Notebook, Orientation, PackType, Popover, PositionType,
    ScrolledWindow, WindowControls,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use webkit6::prelude::*;
use webkit6::{
    CacheModel, CookieAcceptPolicy, CookiePersistentStorage, Download, FindOptions,
    GeolocationPermissionRequest, LoadEvent, MemoryPressureSettings, NetworkSession,
    NotificationPermissionRequest, PermissionRequest, Settings, UserContentInjectedFrames,
    UserContentManager, UserMediaPermissionRequest, UserScript, UserScriptInjectionTime,
    UserStyleLevel, UserStyleSheet, WebContext, WebView,
};

const APP_ID: &str = "dev.ghanti.ubar";
const TAB_WIDTH: i32 = 220;
const TAB_HEIGHT: i32 = 32;
const TAB_TITLE_WIDTH: i32 = 132;
const TAB_ACTIONS_WIDTH: i32 = 44;
const TAB_ANIMATION_MS: u32 = 140;

#[derive(Clone)]
struct TabState {
    web_view: WebView,
    tab_box: GtkBox,
    favicon: Image,
    title: Label,
    audio_icon: Image,
    loading: Rc<Cell<bool>>,
}

struct AppState {
    window: ApplicationWindow,
    notebook: Notebook,
    tab_bar: GtkBox,
    tab_scroller: ScrolledWindow,
    back_button: Button,
    forward_button: Button,
    reload_button: Button,
    address_entry: Entry,
    bookmark_button: Button,
    new_tab_button: Button,
    menu_button: MenuButton,
    find_bar: GtkBox,
    find_entry: Entry,
    network_session: NetworkSession,
    extensions: RefCell<crate::extensions::Extensions>,
    closed_tabs: RefCell<Vec<String>>,
    browser_state: RefCell<BrowserState>,
    vault: RefCell<Vault>,
    active_downloads: RefCell<Vec<(u64, Download)>>,
    new_tab_uri: String,
    history_uri: String,
    bookmarks_uri: String,
    settings_uri: String,
    extensions_uri: String,
    downloads_uri: String,
}

fn is_internal_uri(app: &AppState, uri: &str) -> bool {
    uri == app.new_tab_uri
        || uri == app.history_uri
        || uri == app.bookmarks_uri
        || uri == app.settings_uri
        || uri == app.extensions_uri
        || uri == app.downloads_uri
}

// scheme://host[:port] of a URI, for permission and credential keys.
fn origin_of(uri: &str) -> String {
    let Some(scheme_end) = uri.find("://") else {
        return String::new();
    };
    let rest = &uri[scheme_end + 3..];
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    format!("{}{}", &uri[..scheme_end + 3], &rest[..host_end])
}

// Stores sniff the UA to decide which install button to show.
fn apply_site_user_agent(view: &WebView, uri: &str) {
    // WidgetExt::settings vs WebViewExt::settings are both in scope (E0034).
    let Some(settings) = webkit6::prelude::WebViewExt::settings(view) else {
        return;
    };
    let user_agent = crate::store_identity::for_uri(uri).map(crate::store_identity::user_agent);
    settings.set_user_agent(user_agent);
}

fn search_uri(engine: &str, query: &str) -> String {
    let query = glib::Uri::escape_string(query, None, false);
    match engine {
        "google" => format!("https://www.google.com/search?q={query}"),
        "bing" => format!("https://www.bing.com/search?q={query}"),
        "yandex" => format!("https://yandex.com/search/?text={query}"),
        _ => format!("https://duckduckgo.com/?q={query}"),
    }
}

// Tune WebKit memory use to the host. Must run before any web process spawns.
fn tune_memory_for_host() {
    const MIB: u64 = 1024 * 1024;
    let policy = crate::memory_policy::MemoryPolicy::detect();
    let cache = if policy.constrained {
        CacheModel::DocumentViewer
    } else if policy.physical_bytes <= 2 * 1024 * MIB {
        CacheModel::DocumentBrowser
    } else {
        CacheModel::WebBrowser
    };
    let per_renderer = (policy.elastic_ceiling_bytes / u64::from(policy.renderer_limit)) / MIB;
    let limit = per_renderer.clamp(256, 4096) as u32;

    if let Some(context) = WebContext::default() {
        context.set_cache_model(cache);
    }

    // Per-web-process cap; thresholds are fractions of `limit` (MB).
    let mut settings = MemoryPressureSettings::new();
    settings.set_memory_limit(limit);
    settings.set_conservative_threshold(0.33);
    settings.set_strict_threshold(0.5);
    settings.set_kill_threshold(0.95);
    settings.set_poll_interval(30.0);
    NetworkSession::set_memory_pressure_settings(&mut settings);
}

fn normalize_uri(input: &str, engine: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains("://") {
        return Some(trimmed.to_string());
    }

    // Looks like a search query, not a host -> web search.
    if trimmed.contains(' ') || (!trimmed.contains('.') && !trimmed.contains(':')) {
        return Some(search_uri(engine, trimmed));
    }

    Some(format!("https://{trimmed}"))
}

fn js_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "")
}

fn current_uri(view: &WebView) -> String {
    view.uri().map(|v| v.to_string()).unwrap_or_default()
}

fn current_title(view: &WebView) -> String {
    view.title().map(|v| v.to_string()).unwrap_or_default()
}

fn current_tab(app: &AppState) -> Option<TabState> {
    let page_num = app
        .notebook
        .current_page()
        .or_else(|| (app.notebook.n_pages() > 0).then_some(0))?;
    let page = app.notebook.nth_page(Some(page_num))?;
    unsafe { page.data::<TabState>("tab-state") }
        .map(|ptr| unsafe { ptr.as_ref().clone() })
}

fn focus_address_bar(app: &AppState) {
    app.address_entry.grab_focus();
    app.address_entry.select_region(0, -1);
}

fn find_search(app: &AppState) {
    let text = app.find_entry.text();
    if let Some(tab) = current_tab(app)
        && let Some(finder) = tab.web_view.find_controller()
    {
        if text.is_empty() {
            finder.search_finish();
        } else {
            finder.search(
                &text,
                (FindOptions::CASE_INSENSITIVE | FindOptions::WRAP_AROUND).bits(),
                u32::MAX,
            );
        }
    }
}

fn open_find_bar(app: &AppState) {
    app.find_bar.set_visible(true);
    app.find_entry.grab_focus();
    app.find_entry.select_region(0, -1);
}

fn close_find_bar(app: &AppState) {
    app.find_bar.set_visible(false);
    if let Some(tab) = current_tab(app)
        && let Some(finder) = tab.web_view.find_controller()
    {
        finder.search_finish();
    }
    tab_grab_focus(app);
}

fn tab_grab_focus(app: &AppState) {
    if let Some(tab) = current_tab(app) {
        tab.web_view.grab_focus();
    }
}

fn zoom_current_tab(app: &AppState, delta: f64) {
    if let Some(tab) = current_tab(app) {
        let level = if delta == 0.0 {
            crate::zoom::DEFAULT_ZOOM
        } else if delta > 0.0 {
            crate::zoom::increase(tab.web_view.zoom_level())
        } else {
            crate::zoom::decrease(tab.web_view.zoom_level())
        };
        tab.web_view.set_zoom_level(level);
        let uri = current_uri(&tab.web_view);
        app.browser_state.borrow_mut().set_zoom_for_uri(&uri, level);
    }
}

fn reopen_closed_tab(app: &Rc<AppState>) {
    let Some(uri) = app.closed_tabs.borrow_mut().pop() else {
        return;
    };
    let tab = create_tab(app, &uri);
    if let Some(index) = app.notebook.page_num(&tab.web_view) {
        app.notebook.set_current_page(Some(index));
        sync_window(app, &tab);
        scroll_tab_strip_to_end(app);
    }
}

fn scroll_tab_strip_to_end(app: &AppState) {
    let scroller = app.tab_scroller.clone();
    glib::idle_add_local_once(move || {
        let adj = scroller.hadjustment();
        let target = (adj.upper() - adj.page_size()).max(adj.lower());
        adj.set_value(target);
    });
}

fn scroll_tab_into_view(app: &AppState, tab_box: &GtkBox) {
    let scroller = app.tab_scroller.clone();
    let tab_box = tab_box.clone();
    glib::idle_add_local_once(move || {
        let adj = scroller.hadjustment();
        let x = f64::from(tab_box.allocation().x());
        let width = f64::from(tab_box.allocation().width());
        let start = adj.value();
        let end = start + adj.page_size();

        let target = if x < start {
            x
        } else if x + width > end {
            x + width - adj.page_size()
        } else {
            return;
        };

        let clamped = target.clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower()));
        adj.set_value(clamped);
    });
}

fn sync_tab_bar_order(app: &Rc<AppState>) {
    let total = app.notebook.n_pages();
    let mut previous: Option<GtkBox> = None;
    for index in 0..total {
        if let Some(page) = app.notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page
                .data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
        {
            app.tab_bar.reorder_child_after(&tab.tab_box, previous.as_ref());
            previous = Some(tab.tab_box);
        }
    }
}

fn update_active_tab_styles(notebook: &Notebook, active_index: u32) {
    let total = notebook.n_pages();
    for index in 0..total {
        if let Some(page) = notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page
                .data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
        {
            if index == active_index {
                tab.tab_box.add_css_class("ubar-tab-active");
            } else {
                tab.tab_box.remove_css_class("ubar-tab-active");
            }
        }
    }
}

fn select_tab(app: &Rc<AppState>, index: u32) {
    if index >= app.notebook.n_pages() {
        return;
    }

    app.notebook.set_current_page(Some(index));
    update_active_tab_styles(&app.notebook, index);
    if let Some(page) = app.notebook.nth_page(Some(index))
        && let Some(tab) = unsafe { page
            .data::<TabState>("tab-state") }
            .map(|ptr| unsafe { ptr.as_ref().clone() })
    {
        sync_window(app, &tab);
        scroll_tab_into_view(app, &tab.tab_box);
    }
}

fn move_tab(app: &Rc<AppState>, source: u32, target: u32) {
    if source == target || source >= app.notebook.n_pages() || target >= app.notebook.n_pages() {
        return;
    }

    let Some(page) = app.notebook.nth_page(Some(source)) else {
        return;
    };

    app.notebook.reorder_child(&page, Some(target));
    sync_tab_bar_order(app);
}

fn update_bookmark_button(app: &AppState) {
    if let Some(tab) = current_tab(app) {
        let uri = current_uri(&tab.web_view);
        if uri.is_empty() || is_internal_uri(app, &uri) {
            app.bookmark_button.set_sensitive(false);
            app.bookmark_button.set_icon_name("bookmark-new-symbolic");
            return;
        }

        let bookmarked = app.browser_state.borrow().is_bookmarked(&uri);
        app.bookmark_button.set_sensitive(true);
        app.bookmark_button
            .set_icon_name(if bookmarked { "starred-symbolic" } else { "bookmark-new-symbolic" });
        return;
    }

    app.bookmark_button.set_sensitive(false);
}

fn sync_window(app: &AppState, tab: &TabState) {
    let uri = current_uri(&tab.web_view);
    let title = current_title(&tab.web_view);

    if uri == app.new_tab_uri {
        app.address_entry.set_text("");
    } else {
        app.address_entry.set_text(&uri);
    }

    app.back_button.set_sensitive(tab.web_view.can_go_back());
    app.forward_button.set_sensitive(tab.web_view.can_go_forward());
    app.reload_button.set_icon_name(if tab.loading.get() {
        "process-stop-symbolic"
    } else {
        "view-refresh-symbolic"
    });
    app.window.set_title(Some(if title.is_empty() { "ubar" } else { &title }));
    update_bookmark_button(app);
}

fn configure_web_view(web_view: &WebView, prefs: &SettingsData) {
    let settings = Settings::new();
    settings.set_hardware_acceleration_policy(webkit6::HardwareAccelerationPolicy::Always);
    settings.set_enable_webgl(true);
    settings.set_enable_write_console_messages_to_stdout(true);
    settings.set_enable_developer_extras(true);
    // Let WebKit drop decoded resources and page caches under memory pressure.
    settings.set_enable_page_cache(true);
    settings.set_default_font_size(prefs.font_size);
    web_view.set_settings(&settings);
    web_view.set_zoom_level(crate::zoom::clamp(prefs.default_zoom));

    if let Some(session) = web_view.network_session() {
        if let Some(data_manager) = session.website_data_manager() {
            data_manager.set_favicons_enabled(true);
        }
    }
}

fn apply_theme(theme: &str) {
    if let Some(settings) = gtk4::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(theme == "dark");
    }
}

// Re-apply zoom/font settings to every open tab after a settings change.
fn apply_appearance(app: &Rc<AppState>) {
    let prefs = app.browser_state.borrow().settings.clone();
    apply_theme(&prefs.theme);
    for index in 0..app.notebook.n_pages() {
        if let Some(page) = app.notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page.data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
        {
            tab.web_view.set_zoom_level(crate::zoom::clamp(prefs.default_zoom));
            if let Some(settings) = webkit6::prelude::WebViewExt::settings(&tab.web_view) {
                settings.set_default_font_size(prefs.font_size);
            }
        }
    }
}

fn apply_cookie_policy(app: &AppState) {
    if let Some(cookies) = app.network_session.cookie_manager() {
        let rule = app.browser_state.borrow().permission_for("", "cookies");
        cookies.set_accept_policy(match rule.as_str() {
            "allow" => CookieAcceptPolicy::Always,
            "block" => CookieAcceptPolicy::Never,
            _ => CookieAcceptPolicy::NoThirdParty,
        });
    }
}

fn update_tab_title(app: &AppState, tab: &TabState) {
    let uri = current_uri(&tab.web_view);
    let title = current_title(&tab.web_view);
    let text = if uri == app.new_tab_uri {
        "New Tab".to_string()
    } else if uri == app.history_uri {
        "History".to_string()
    } else if uri == app.bookmarks_uri {
        "Bookmarks".to_string()
    } else if uri == app.settings_uri {
        "Settings".to_string()
    } else if uri == app.extensions_uri {
        "Extensions".to_string()
    } else if uri == app.downloads_uri {
        "Downloads".to_string()
    } else if !title.is_empty() {
        title
    } else if !uri.is_empty() {
        uri
    } else {
        "New Tab".to_string()
    };
    tab.title.set_label(&text);
}

fn update_tab_favicon(tab: &TabState) {
    if let Some(texture) = tab.web_view.favicon() {
        tab.favicon.set_paintable(Some(&texture));
    } else {
        tab.favicon.set_icon_name(Some("globe-symbolic"));
    }
}

fn update_tab_audio(tab: &TabState) {
    let playing = tab.web_view.is_playing_audio();
    tab.audio_icon.set_visible(playing);
    if playing {
        tab.audio_icon.set_icon_name(Some(if tab.web_view.is_muted() {
            "audio-volume-muted-symbolic"
        } else {
            "audio-volume-high-symbolic"
        }));
    }
}

fn evaluate_js(view: &WebView, script: &str, source_uri: &str) {
    view.evaluate_javascript(
        script,
        None::<&str>,
        Some(source_uri),
        gio::Cancellable::NONE,
        |_| {},
    );
}

fn build_history_script(state: &BrowserState) -> String {
    let items = state
        .history
        .iter()
        .map(|entry| {
            format!(
                "{{title:'{}',uri:'{}',time:'{}'}}",
                js_escape(&entry.title),
                js_escape(&entry.uri),
                js_escape(&entry.timestamp.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("window.ubarRenderHistory({{items:[{items}]}});")
}

fn build_bookmarks_script(state: &BrowserState) -> String {
    let items = state
        .bookmarks
        .iter()
        .map(|entry| {
            format!(
                "{{title:'{}',uri:'{}'}}",
                js_escape(&entry.title),
                js_escape(&entry.uri)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("window.ubarRenderBookmarks({{items:[{items}]}});")
}

fn build_settings_script(state: &BrowserState, vault: &Vault) -> String {
    let defaults = state
        .permission_defaults
        .iter()
        .map(|(key, value)| format!("'{}':'{}'", js_escape(key), js_escape(value)))
        .collect::<Vec<_>>()
        .join(",");
    let sites = state
        .site_permissions
        .iter()
        .map(|(origin, rules)| {
            let mut fields = vec![format!("origin:'{}'", js_escape(origin))];
            fields.extend(
                rules
                    .iter()
                    .map(|(key, value)| format!("'{}':'{}'", js_escape(key), js_escape(value))),
            );
            format!("{{{}}}", fields.join(","))
        })
        .collect::<Vec<_>>()
        .join(",");
    let passwords = vault
        .entries()
        .iter()
        .map(|entry| {
            format!(
                "{{origin:'{}',username:'{}'}}",
                js_escape(&entry.origin),
                js_escape(&entry.username)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "window.ubarRenderSettings({{homepageUri:'{}',historyCount:{},bookmarkCount:{},searchEngine:'{}',downloadDir:'{}',theme:'{}',defaultZoom:{},fontSize:{},defaults:{{{defaults}}},sites:[{sites}],passwords:[{passwords}]}});",
        js_escape(&state.settings.homepage_uri),
        state.history.len(),
        state.bookmarks.len(),
        js_escape(&state.settings.search_engine),
        js_escape(&state.settings.download_dir),
        js_escape(&state.settings.theme),
        state.settings.default_zoom,
        state.settings.font_size,
    )
}

fn build_downloads_script(state: &BrowserState) -> String {
    let items = state
        .downloads
        .iter()
        .map(|entry| {
            format!(
                "{{id:{},filename:'{}',uri:'{}',destination:'{}',received:{},total:{},status:'{}',time:'{}'}}",
                entry.id,
                js_escape(&entry.filename),
                js_escape(&entry.uri),
                js_escape(&entry.destination),
                entry.received,
                entry.total,
                js_escape(&entry.status),
                entry.timestamp,
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("window.ubarRenderDownloads({{items:[{items}]}});")
}

fn build_extensions_script(app: &AppState) -> String {
    let extensions = app.extensions.borrow();
    let items = extensions
        .names
        .iter()
        .zip(extensions.dirs.iter())
        .zip(extensions.pages.iter())
        .map(|((name, dir), page)| {
            format!(
                "{{name:'{}',dir:'{}',page:'{}'}}",
                js_escape(name),
                js_escape(&dir.to_string_lossy()),
                js_escape(page)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("window.ubarRenderExtensions({{items:[{items}]}});")
}

fn render_internal_page(app: &Rc<AppState>, tab: &TabState) {
    let uri = current_uri(&tab.web_view);
    let state = app.browser_state.borrow();
    if uri == app.history_uri {
        evaluate_js(&tab.web_view, &build_history_script(&state), &app.history_uri);
    } else if uri == app.bookmarks_uri {
        evaluate_js(&tab.web_view, &build_bookmarks_script(&state), &app.bookmarks_uri);
    } else if uri == app.settings_uri {
        evaluate_js(
            &tab.web_view,
            &build_settings_script(&state, &app.vault.borrow()),
            &app.settings_uri,
        );
    } else if uri == app.extensions_uri {
        evaluate_js(&tab.web_view, &build_extensions_script(app), &app.extensions_uri);
    } else if uri == app.downloads_uri {
        evaluate_js(&tab.web_view, &build_downloads_script(&state), &app.downloads_uri);
    }
}

fn animate_tab_opacity(tab_box: &GtkBox, from: f64, to: f64) {
    let tab_box = tab_box.clone();
    let start = std::time::Instant::now();
    tab_box.set_opacity(from);

    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let elapsed = start.elapsed().as_millis() as u32;
        let progress = (elapsed.min(TAB_ANIMATION_MS) as f64) / (TAB_ANIMATION_MS as f64);
        let eased = 1.0 - (1.0 - progress) * (1.0 - progress);
        let opacity = from + ((to - from) * eased);
        tab_box.set_opacity(opacity);

        if elapsed >= TAB_ANIMATION_MS {
            tab_box.set_opacity(to);
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

fn refresh_internal_pages(app: &Rc<AppState>) {
    let total = app.notebook.n_pages();
    for index in 0..total {
        if let Some(page) = app.notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page
                .data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
        {
            let uri = current_uri(&tab.web_view);
            if !uri.is_empty() && uri != app.new_tab_uri && is_internal_uri(app, &uri) {
                render_internal_page(app, &tab);
            }
        }
    }
}

fn close_tab(app: &Rc<AppState>, tab: &TabState) {
    let Some(page_num) = app.notebook.page_num(&tab.web_view) else {
        return;
    };
    let current_page = app.notebook.current_page();
    let page_count = app.notebook.n_pages();

    let closed_uri = current_uri(&tab.web_view);
    if !closed_uri.is_empty() && closed_uri != app.new_tab_uri {
        app.closed_tabs.borrow_mut().push(closed_uri);
    }

    if page_count == 1 {
        app.window.close();
        return;
    }

    tab.tab_box.set_sensitive(false);
    animate_tab_opacity(&tab.tab_box, 1.0, 0.0);
    let closing_current = current_page == Some(page_num);
    let next = if page_num == page_count - 1 {
        page_num.saturating_sub(1)
    } else {
        page_num + 1
    };

    let notebook = app.notebook.clone();
    let tab_bar = app.tab_bar.clone();
    let view = tab.web_view.clone();
    let tab_box = tab.tab_box.clone();
    glib::timeout_add_local_once(std::time::Duration::from_millis(TAB_ANIMATION_MS as u64), move || {
        if let Some(index) = notebook.page_num(&view) {
            notebook.remove_page(Some(index));
        }
        tab_bar.remove(&tab_box);
        if closing_current {
            notebook.set_current_page(Some(next));
        }
    });
}

fn navigate_tab(tab: &TabState, uri: &str) {
    let web_view = tab.web_view.clone();
    let target = uri.to_string();
    if tab.loading.get() {
        web_view.stop_loading();
    }
    glib::idle_add_local_once(move || {
        apply_site_user_agent(&web_view, &target);
        web_view.load_uri(&target);
    });
}

fn remove_tab_at(app: &Rc<AppState>, index: u32) {
    let Some(page) = app.notebook.nth_page(Some(index)) else {
        return;
    };
    let tab = unsafe { page.data::<TabState>("tab-state") }
        .map(|ptr| unsafe { ptr.as_ref().clone() });
    app.notebook.remove_page(Some(index));
    if let Some(tab) = tab {
        app.tab_bar.remove(&tab.tab_box);
    }
}

fn duplicate_tab(app: &Rc<AppState>, source: &TabState) {
    let uri = current_uri(&source.web_view);
    let target = if uri.is_empty() {
        app.new_tab_uri.clone()
    } else {
        uri
    };
    let tab = create_tab(app, &target);
    if let Some(index) = app.notebook.page_num(&tab.web_view) {
        app.notebook.set_current_page(Some(index));
        sync_window(app, &tab);
        scroll_tab_strip_to_end(app);
    }
}

fn close_tabs_to_right(app: &Rc<AppState>, source: &TabState) {
    let Some(current) = app.notebook.page_num(&source.web_view) else {
        return;
    };
    let total = app.notebook.n_pages();
    for index in (current + 1..total).rev() {
        remove_tab_at(app, index);
    }
    sync_tab_bar_order(app);
    select_tab(app, current);
}

fn close_other_tabs(app: &Rc<AppState>, source: &TabState) {
    let Some(current) = app.notebook.page_num(&source.web_view) else {
        return;
    };
    let total = app.notebook.n_pages();
    for index in (current + 1..total).rev() {
        remove_tab_at(app, index);
    }
    for index in (0..current).rev() {
        remove_tab_at(app, index);
    }
    sync_tab_bar_order(app);
    if let Some(index) = app.notebook.page_num(&source.web_view) {
        select_tab(app, index);
    }
}

fn open_tab_context_menu(app: &Rc<AppState>, tab: &TabState, x: f64, y: f64) {
    if let Some(index) = app.notebook.page_num(&tab.web_view) {
        app.notebook.set_current_page(Some(index));
    }

    let menu_box = GtkBox::new(Orientation::Vertical, 4);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);

    let duplicate_button = Button::with_label("Duplicate Tab");
    let close_button = Button::with_label("Close Tab");
    let close_others_button = Button::with_label("Close Other Tabs");
    let close_right_button = Button::with_label("Close Tabs to Right");
    for button in [&duplicate_button, &close_button, &close_others_button, &close_right_button] {
        button.set_has_frame(false);
        button.set_halign(Align::Fill);
        menu_box.append(button);
    }

    let mute_button = if tab.web_view.is_playing_audio() {
        let button = Button::with_label(if tab.web_view.is_muted() {
            "Unmute Tab"
        } else {
            "Mute Tab"
        });
        button.set_has_frame(false);
        button.set_halign(Align::Fill);
        menu_box.append(&button);
        Some(button)
    } else {
        None
    };

    let popover = Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(PositionType::Bottom);
    popover.set_child(Some(&menu_box));
    popover.set_parent(&tab.tab_box);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|popover| popover.unparent());

    let pop_duplicate = popover.clone();
    let app_duplicate = app.clone();
    let tab_duplicate = tab.clone();
    duplicate_button.connect_clicked(move |_| {
        pop_duplicate.popdown();
        duplicate_tab(&app_duplicate, &tab_duplicate);
    });

    let pop_close = popover.clone();
    let app_close = app.clone();
    let tab_close = tab.clone();
    close_button.connect_clicked(move |_| {
        pop_close.popdown();
        close_tab(&app_close, &tab_close);
    });

    let pop_close_others = popover.clone();
    let app_close_others = app.clone();
    let tab_close_others = tab.clone();
    close_others_button.connect_clicked(move |_| {
        pop_close_others.popdown();
        close_other_tabs(&app_close_others, &tab_close_others);
    });

    let pop_close_right = popover.clone();
    let app_close_right = app.clone();
    let tab_close_right = tab.clone();
    close_right_button.connect_clicked(move |_| {
        pop_close_right.popdown();
        close_tabs_to_right(&app_close_right, &tab_close_right);
    });

    if let Some(mute_button) = mute_button {
        let pop_mute = popover.clone();
        let tab_mute = tab.clone();
        mute_button.connect_clicked(move |_| {
            pop_mute.popdown();
            tab_mute.web_view.set_is_muted(!tab_mute.web_view.is_muted());
            update_tab_audio(&tab_mute);
        });
    }

    popover.popup();
}

fn load_uri_in_current_tab(app: &Rc<AppState>, uri: &str) {
    if let Some(tab) = current_tab(app) {
        if let Some(index) = app.notebook.page_num(&tab.web_view) {
            app.notebook.set_current_page(Some(index));
            update_active_tab_styles(&app.notebook, index);
        }
        navigate_tab(&tab, uri);
        return;
    }

    if app.notebook.n_pages() > 0 {
        select_tab(app, 0);
        if let Some(tab) = current_tab(app) {
            navigate_tab(&tab, uri);
        }
    }
}

fn handle_script_message(app: &Rc<AppState>, view: &WebView, message: &str) {
    let uri = current_uri(view);
    if let Some(rest) = message.strip_prefix("cred:") {
        handle_credential_capture(app, rest, &origin_of(&uri));
        return;
    }
    if !is_internal_uri(app, &uri) {
        return;
    }

    if let Some(uri) = message.strip_prefix("open:") {
        let decoded = glib::uri_unescape_string(uri, None::<&str>).unwrap_or_default();
        let app_open = app.clone();
        glib::idle_add_local_once(move || load_uri_in_current_tab(&app_open, &decoded));
        return;
    }

    if message == "clear-history" {
        app.browser_state.borrow_mut().clear_history();
        refresh_internal_pages(app);
        return;
    }

    if let Some(uri) = message.strip_prefix("delete-bookmark:") {
        let decoded = glib::uri_unescape_string(uri, None::<&str>).unwrap_or_default();
        if app.browser_state.borrow_mut().remove_bookmark(&decoded) {
            refresh_internal_pages(app);
            update_bookmark_button(app);
        }
        return;
    }

    if let Some(name) = message.strip_prefix("delete-extension:") {
        let decoded = glib::uri_unescape_string(name, None::<&str>).unwrap_or_default();
        let removed = crate::extensions::uninstall(&decoded, &app.extensions.borrow());
        if removed {
            *app.extensions.borrow_mut() = crate::extensions::load();
            refresh_internal_pages(app);
        }
        return;
    }

    if let Some(uri) = message.strip_prefix("save-homepage:") {
        let decoded = glib::uri_unescape_string(uri, None::<&str>).unwrap_or_default();
        let engine = app.browser_state.borrow().settings.search_engine.clone();
        app.browser_state.borrow_mut().settings.homepage_uri =
            normalize_uri(&decoded, &engine).unwrap_or_default();
        app.browser_state.borrow().save();
        refresh_internal_pages(app);
        return;
    }

    if let Some(engine) = message.strip_prefix("save-search-engine:") {
        app.browser_state.borrow_mut().settings.search_engine = engine.to_string();
        app.browser_state.borrow().save();
        return;
    }

    if let Some(dir) = message.strip_prefix("save-download-dir:") {
        let decoded = glib::uri_unescape_string(dir, None::<&str>).unwrap_or_default();
        app.browser_state.borrow_mut().settings.download_dir = decoded.to_string();
        app.browser_state.borrow().save();
        refresh_internal_pages(app);
        return;
    }

    if let Some(theme) = message.strip_prefix("save-theme:") {
        app.browser_state.borrow_mut().settings.theme = theme.to_string();
        app.browser_state.borrow().save();
        apply_appearance(app);
        return;
    }

    if let Some(zoom) = message.strip_prefix("save-zoom:") {
        if let Ok(value) = zoom.parse::<f64>() {
            app.browser_state.borrow_mut().settings.default_zoom = crate::zoom::clamp(value);
            app.browser_state.borrow().save();
            apply_appearance(app);
        }
        return;
    }

    if let Some(size) = message.strip_prefix("save-font-size:") {
        if let Ok(value) = size.parse::<u32>() {
            app.browser_state.borrow_mut().settings.font_size = value.clamp(6, 72);
            app.browser_state.borrow().save();
            apply_appearance(app);
        }
        return;
    }

    if let Some(rest) = message.strip_prefix("save-permission-default:") {
        if let Some((key, value)) = rest.split_once(':') {
            app.browser_state
                .borrow_mut()
                .permission_defaults
                .insert(key.to_string(), value.to_string());
            app.browser_state.borrow().save();
            apply_cookie_policy(app);
            refresh_internal_pages(app);
        }
        return;
    }

    if let Some(rest) = message.strip_prefix("save-site-permission:") {
        let mut parts = rest.splitn(3, ':');
        if let (Some(origin), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next()) {
            let origin = glib::uri_unescape_string(origin, None::<&str>).unwrap_or_default();
            app.browser_state
                .borrow_mut()
                .set_site_permission(&origin, key, value);
            refresh_internal_pages(app);
        }
        return;
    }

    if let Some(origin) = message.strip_prefix("remove-site:") {
        let decoded = glib::uri_unescape_string(origin, None::<&str>).unwrap_or_default();
        app.browser_state
            .borrow_mut()
            .site_permissions
            .remove(decoded.as_str());
        app.browser_state.borrow().save();
        refresh_internal_pages(app);
        return;
    }

    if let Some(origin) = message.strip_prefix("delete-credential:") {
        let decoded = glib::uri_unescape_string(origin, None::<&str>).unwrap_or_default();
        if app.vault.borrow_mut().remove(&decoded) {
            refresh_internal_pages(app);
        }
        return;
    }

    if let Some(id) = message.strip_prefix("download-open:") {
        if let Ok(id) = id.parse::<u64>() {
            let destination = app
                .browser_state
                .borrow()
                .downloads
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.destination.clone());
            if let Some(destination) = destination {
                let _ = gio::AppInfo::launch_default_for_uri(
                    &format!("file://{destination}"),
                    None::<&gio::AppLaunchContext>,
                );
            }
        }
        return;
    }

    if let Some(id) = message.strip_prefix("download-cancel:") {
        if let Ok(id) = id.parse::<u64>() {
            let download = app
                .active_downloads
                .borrow()
                .iter()
                .find(|(entry_id, _)| *entry_id == id)
                .map(|(_, download)| download.clone());
            if let Some(download) = download {
                download.cancel();
            }
        }
        return;
    }

    if message == "downloads-clear" {
        let mut state = app.browser_state.borrow_mut();
        state.downloads.retain(|entry| entry.status == "active");
        state.save();
        drop(state);
        refresh_internal_pages(app);
        return;
    }

    if message == "downloads-folder" {
        let dir = download_directory(app);
        let _ = gio::AppInfo::launch_default_for_uri(
            &format!("file://{}", dir.to_string_lossy()),
            None::<&gio::AppLaunchContext>,
        );
    }
}

fn download_directory(app: &AppState) -> std::path::PathBuf {
    let configured = app.browser_state.borrow().settings.download_dir.clone();
    if !configured.is_empty() {
        let path = std::path::PathBuf::from(&configured);
        if std::fs::create_dir_all(&path).is_ok() {
            return path;
        }
    }
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn refresh_downloads_pages(app: &Rc<AppState>) {
    let script = build_downloads_script(&app.browser_state.borrow());
    for index in 0..app.notebook.n_pages() {
        if let Some(page) = app.notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page.data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
            && current_uri(&tab.web_view) == app.downloads_uri
        {
            evaluate_js(&tab.web_view, &script, &app.downloads_uri);
        }
    }
}

// Fill the first login form when a credential is stored for this origin.
fn autofill_credentials(app: &Rc<AppState>, view: &WebView) {
    let origin = origin_of(&current_uri(view));
    if origin.is_empty() {
        return;
    }
    let Some((username, password)) = app.vault.borrow().get(&origin) else {
        return;
    };
    let script = format!(
        r#"(function(){{
var d=function(v){{return decodeURIComponent(escape(atob(v)))}};
var u=d('{}'),p=d('{}');
var pw=document.querySelector('input[type=password]');
if(!pw)return;
var form=pw.form;
if(form&&u){{var user=form.querySelector('input[type=email],input[type=text]');if(user&&!user.value)user.value=u;}}
if(!pw.value)pw.value=p;
}})();"#,
        B64.encode(username.as_bytes()),
        B64.encode(password.as_bytes()),
    );
    evaluate_js(view, &script, "ubar://autofill");
}

// cred:<b64 origin>:<b64 user>:<b64 pass> from the capture user script.
fn handle_credential_capture(app: &Rc<AppState>, payload: &str, expected_origin: &str) {
    let mut parts = payload.splitn(3, ':');
    let (Some(origin), Some(user), Some(pass)) = (parts.next(), parts.next(), parts.next()) else {
        return;
    };
    let decode = |value: &str| {
        B64.decode(value)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    let (Some(origin), Some(username), Some(password)) = (decode(origin), decode(user), decode(pass))
    else {
        return;
    };
    if origin.is_empty()
        || origin != expected_origin
        || password.is_empty()
        || app.vault.borrow().is_never(&origin)
    {
        return;
    }
    if app.vault.borrow().get(&origin).map(|(u, p)| (u, p)) == Some((username.clone(), password.clone())) {
        return;
    }

    let menu_box = GtkBox::new(Orientation::Vertical, 8);
    menu_box.set_margin_top(10);
    menu_box.set_margin_bottom(10);
    menu_box.set_margin_start(10);
    menu_box.set_margin_end(10);
    let label = Label::new(Some(&format!("Save password for {origin}?")));
    let save_button = Button::with_label("Save");
    let never_button = Button::with_label("Never for this site");
    menu_box.append(&label);
    menu_box.append(&save_button);
    menu_box.append(&never_button);

    let popover = Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(&app.address_entry);
    popover.set_position(PositionType::Bottom);
    popover.connect_closed(|popover| popover.unparent());

    let app_save = app.clone();
    let pop_save = popover.clone();
    let origin_save = origin.clone();
    save_button.connect_clicked(move |_| {
        app_save
            .vault
            .borrow_mut()
            .save(&origin_save, &username, &password);
        refresh_internal_pages(&app_save);
        pop_save.popdown();
    });

    let app_never = app.clone();
    let pop_never = popover.clone();
    never_button.connect_clicked(move |_| {
        app_never.vault.borrow_mut().set_never(&origin);
        pop_never.popdown();
    });

    popover.popup();
}

// Ask the user, remember the answer as a site rule.
fn prompt_permission(app: &Rc<AppState>, origin: String, key: &'static str, request: PermissionRequest) {
    let menu_box = GtkBox::new(Orientation::Vertical, 8);
    menu_box.set_margin_top(10);
    menu_box.set_margin_bottom(10);
    menu_box.set_margin_start(10);
    menu_box.set_margin_end(10);
    let label = Label::new(Some(&format!("{origin} wants to use: {key}")));
    let allow_button = Button::with_label("Allow");
    let block_button = Button::with_label("Block");
    menu_box.append(&label);
    menu_box.append(&allow_button);
    menu_box.append(&block_button);

    let popover = Popover::new();
    popover.set_child(Some(&menu_box));
    popover.set_parent(&app.address_entry);
    popover.set_position(PositionType::Bottom);

    let decided = Rc::new(Cell::new(false));

    let app_allow = app.clone();
    let pop_allow = popover.clone();
    let request_allow = request.clone();
    let origin_allow = origin.clone();
    let decided_allow = decided.clone();
    allow_button.connect_clicked(move |_| {
        decided_allow.set(true);
        request_allow.allow();
        app_allow
            .browser_state
            .borrow_mut()
            .set_site_permission(&origin_allow, key, "allow");
        refresh_internal_pages(&app_allow);
        pop_allow.popdown();
    });

    let app_block = app.clone();
    let pop_block = popover.clone();
    let request_block = request.clone();
    let decided_block = decided.clone();
    block_button.connect_clicked(move |_| {
        decided_block.set(true);
        request_block.deny();
        app_block
            .browser_state
            .borrow_mut()
            .set_site_permission(&origin, key, "block");
        refresh_internal_pages(&app_block);
        pop_block.popdown();
    });

    popover.connect_closed(move |popover| {
        if !decided.get() {
            request.deny();
        }
        popover.unparent();
    });

    popover.popup();
}

// Injected on the store sites: floating "Install in ubar" button that navigates
// to the .xpi / .crx file, which the download handler routes to the installer.
const AMO_HELPER: &str = r#"(function(){
function addBtn(){
  var old=document.getElementById('ubar-install');
  var link=document.querySelector('a[href*="/downloads/file/"]');
  if(!link){if(old)old.remove();return;}
  if(old){old.dataset.href=link.href;return;}
  var b=document.createElement('button');
  b.id='ubar-install';b.textContent='Install in ubar';b.dataset.href=link.href;
  b.style.cssText='position:fixed;bottom:20px;right:20px;z-index:2147483647;padding:12px 20px;border-radius:999px;border:none;background:#1a73e8;color:#fff;font:600 14px system-ui;cursor:pointer;box-shadow:0 4px 12px rgba(0,0,0,.3)';
  b.onclick=function(){location.href=b.dataset.href};
  document.body.appendChild(b);
}
addBtn();
new MutationObserver(addBtn).observe(document.documentElement,{childList:true,subtree:true});
})();"#;

const CWS_HELPER: &str = r#"(function(){
function addBtn(){
  var old=document.getElementById('ubar-install');
  var m=location.pathname.match(/\/detail\/[^\/]+\/([a-p]{32})/);
  if(!m){if(old)old.remove();return;}
  var url='https://clients2.google.com/service/update2/crx?response=redirect&prodversion=140.0.0.0&acceptformat=crx2,crx3&x=id%3D'+m[1]+'%26uc';
  if(old){old.dataset.href=url;return;}
  var b=document.createElement('button');
  b.id='ubar-install';b.textContent='Install in ubar';b.dataset.href=url;
  b.style.cssText='position:fixed;bottom:20px;right:20px;z-index:2147483647;padding:12px 20px;border-radius:999px;border:none;background:#1a73e8;color:#fff;font:600 14px system-ui;cursor:pointer;box-shadow:0 4px 12px rgba(0,0,0,.3)';
  b.onclick=function(){location.href=b.dataset.href};
  document.body.appendChild(b);
}
addBtn();
new MutationObserver(addBtn).observe(document.documentElement,{childList:true,subtree:true});
})();"#;

fn inject_store_helpers(manager: &UserContentManager) {
    manager.add_script(&UserScript::new(
        AMO_HELPER,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::End,
        &["https://addons.mozilla.org/*"],
        &[],
    ));
    manager.add_script(&UserScript::new(
        CWS_HELPER,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::End,
        &["https://chromewebstore.google.com/*"],
        &[],
    ));
}

fn inject_extensions(app: &AppState, manager: &UserContentManager) {
    let extensions = app.extensions.borrow();
    for script in &extensions.scripts {
        let allow: Vec<&str> = script.allowlist.iter().map(String::as_str).collect();
        let time = if script.at_start {
            UserScriptInjectionTime::Start
        } else {
            UserScriptInjectionTime::End
        };
        manager.add_script(&UserScript::for_world(
            &script.source,
            UserContentInjectedFrames::TopFrame,
            time,
            "ubar-ext",
            &allow,
            &[],
        ));
    }
    for style in &extensions.styles {
        let allow: Vec<&str> = style.allowlist.iter().map(String::as_str).collect();
        manager.add_style_sheet(&UserStyleSheet::new(
            &style.source,
            UserContentInjectedFrames::TopFrame,
            UserStyleLevel::Author,
            &allow,
            &[],
        ));
    }
}

// Posts submitted login forms to the vault as cred:<b64 origin>:<b64 user>:<b64 pass>.
const CRED_CAPTURE: &str = r#"document.addEventListener('submit',function(e){
try{
var f=e.target;if(!f||!f.querySelector)return;
var pw=f.querySelector('input[type=password]');if(!pw||!pw.value)return;
var user=f.querySelector('input[type=email],input[type=text]');
var enc=function(v){return btoa(unescape(encodeURIComponent(v)))};
window.webkit.messageHandlers.ubar.postMessage('cred:'+enc(location.origin)+':'+enc(user?user.value:'')+':'+enc(pw.value));
}catch(err){}
},true);"#;

fn create_tab(app: &Rc<AppState>, uri: &str) -> TabState {
    let manager = UserContentManager::new();
    let _ = manager.register_script_message_handler("ubar", None::<&str>);
    inject_extensions(app, &manager);
    inject_store_helpers(&manager);
    manager.add_script(&UserScript::new(
        CRED_CAPTURE,
        UserContentInjectedFrames::TopFrame,
        UserScriptInjectionTime::End,
        &["https://*/*", "http://*/*"],
        &[],
    ));

    let web_view: WebView = glib::Object::builder()
        .property("user-content-manager", &manager)
        .property("network-session", &app.network_session)
        .build();
    configure_web_view(&web_view, &app.browser_state.borrow().settings);

    let favicon = Image::from_icon_name("globe-symbolic");
    favicon.set_pixel_size(14);
    favicon.set_valign(Align::Center);
    let audio_icon = Image::from_icon_name("audio-volume-high-symbolic");
    audio_icon.add_css_class("ubar-tab-audio");
    audio_icon.set_pixel_size(14);
    audio_icon.set_valign(Align::Center);
    audio_icon.set_visible(false);
    let title = Label::new(Some("New Tab"));
    title.add_css_class("ubar-tab-title");
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_width_chars(16);
    title.set_max_width_chars(16);
    title.set_width_request(TAB_TITLE_WIDTH);
    title.set_xalign(0.0);
    title.set_valign(Align::Center);

    let close_button = Button::from_icon_name("window-close-symbolic");
    close_button.add_css_class("ubar-tab-close");
    close_button.set_has_frame(false);
    close_button.set_focusable(false);
    close_button.set_size_request(22, 22);
    // Push close to the right edge; without this it left-packs in the fixed-width
    // actions box, leaving a gap once the audio icon is hidden.
    close_button.set_hexpand(true);
    close_button.set_halign(Align::End);

    let actions_box = GtkBox::new(Orientation::Horizontal, 8);
    actions_box.set_size_request(TAB_ACTIONS_WIDTH, -1);
    actions_box.set_halign(Align::End);
    actions_box.set_valign(Align::Center);
    actions_box.append(&audio_icon);
    actions_box.append(&close_button);

    let tab_box = GtkBox::new(Orientation::Horizontal, 8);
    tab_box.add_css_class("ubar-tab");
    tab_box.set_margin_top(0);
    tab_box.set_margin_bottom(0);
    tab_box.set_margin_start(6);
    tab_box.set_margin_end(6);
    tab_box.set_halign(Align::Start);
    tab_box.set_valign(Align::Center);
    tab_box.set_size_request(TAB_WIDTH, TAB_HEIGHT);
    tab_box.set_opacity(0.0);
    tab_box.append(&favicon);
    tab_box.append(&title);
    tab_box.append(&actions_box);

    web_view.set_hexpand(true);
    web_view.set_vexpand(true);
    app.notebook.append_page(&web_view, None::<&gtk4::Widget>);
    app.tab_bar.append(&tab_box);

    let tab = TabState {
        web_view: web_view.clone(),
        tab_box: tab_box.clone(),
        favicon: favicon.clone(),
        title: title.clone(),
        audio_icon: audio_icon.clone(),
        loading: Rc::new(Cell::new(false)),
    };

    unsafe {
        web_view.set_data("tab-state", tab.clone());
    }

    let app_close = app.clone();
    let tab_close = tab.clone();
    close_button.connect_clicked(move |_| close_tab(&app_close, &tab_close));

    let click = GestureClick::new();
    click.set_button(gdk::BUTTON_MIDDLE);
    let app_middle = app.clone();
    let tab_middle = tab.clone();
    click.connect_pressed(move |_, _, _, _| close_tab(&app_middle, &tab_middle));
    tab_box.add_controller(click);

    let select_click = GestureClick::new();
    select_click.set_button(gdk::BUTTON_PRIMARY);
    let app_select = app.clone();
    let view_select = web_view.clone();
    select_click.connect_pressed(move |_, _, _, _| {
        if let Some(index) = app_select.notebook.page_num(&view_select) {
            app_select.notebook.set_current_page(Some(index));
        }
    });
    tab_box.add_controller(select_click);

    let context_click = GestureClick::new();
    context_click.set_button(gdk::BUTTON_SECONDARY);
    let app_context = app.clone();
    let tab_context = tab.clone();
    context_click.connect_pressed(move |_, _, x, y| {
        open_tab_context_menu(&app_context, &tab_context, x, y);
    });
    tab_box.add_controller(context_click);

    let drag_source = DragSource::new();
    drag_source.set_actions(gdk::DragAction::MOVE);
    let app_drag = app.clone();
    let view_drag = web_view.clone();
    drag_source.connect_prepare(move |_, _, _| {
        let index = app_drag
            .notebook
            .page_num(&view_drag)
            .map(|index| index as i32)
            .unwrap_or(-1);
        Some(gdk::ContentProvider::for_value(&index.to_value()))
    });
    tab_box.add_controller(drag_source);

    let drop_target = DropTarget::new(i32::static_type(), gdk::DragAction::MOVE);
    let app_drop = app.clone();
    let view_drop = web_view.clone();
    let tab_box_drop = tab_box.clone();
    drop_target.connect_drop(move |_, value, x, _| {
        let Ok(source) = value.get::<i32>() else {
            return false;
        };
        let Some(target) = app_drop.notebook.page_num(&view_drop) else {
            return false;
        };
        if source < 0 {
            return false;
        }

        let width = f64::from(tab_box_drop.allocation().width().max(1));
        let insert_after = x >= width / 2.0;
        let mut destination = target + u32::from(insert_after);
        let page_count = app_drop.notebook.n_pages();
        if destination >= page_count {
            destination = page_count.saturating_sub(1);
        }
        if (source as u32) < destination {
            destination = destination.saturating_sub(1);
        }

        move_tab(&app_drop, source as u32, destination);
        true
    });
    tab_box.add_controller(drop_target);

    let app_uri = app.clone();
    let tab_uri_ref = tab.clone();
    web_view.connect_uri_notify(move |_| {
        update_tab_title(&app_uri, &tab_uri_ref);
        if let Some(current) = current_tab(&app_uri) && current.web_view == tab_uri_ref.web_view {
            sync_window(&app_uri, &tab_uri_ref);
        }
    });

    let app_title = app.clone();
    let tab_title_ref = tab.clone();
    web_view.connect_title_notify(move |_| {
        update_tab_title(&app_title, &tab_title_ref);
        if let Some(current) = current_tab(&app_title) && current.web_view == tab_title_ref.web_view {
            sync_window(&app_title, &tab_title_ref);
        }
    });

    let tab_icon = tab.clone();
    web_view.connect_favicon_notify(move |_| update_tab_favicon(&tab_icon));

    web_view.connect_uri_notify(|view| apply_site_user_agent(view, &current_uri(view)));

    // Camera / microphone / location / notification prompts, honoring stored rules.
    let app_perm = app.clone();
    web_view.connect_permission_request(move |view, request| {
        let origin = origin_of(&current_uri(view));
        let key = if request.is::<NotificationPermissionRequest>() {
            "notifications"
        } else if request.is::<GeolocationPermissionRequest>() {
            "location"
        } else if let Some(media) = request.downcast_ref::<UserMediaPermissionRequest>() {
            if media.is_for_video_device() {
                "camera"
            } else {
                "microphone"
            }
        } else {
            request.deny();
            return true;
        };

        match app_perm.browser_state.borrow().permission_for(&origin, key).as_str() {
            "allow" => request.allow(),
            "block" => request.deny(),
            _ => prompt_permission(&app_perm, origin, key, request.clone()),
        }
        true
    });

    // Per-site JavaScript rule, applied before the page runs scripts.
    let app_js = app.clone();
    web_view.connect_uri_notify(move |view| {
        let origin = origin_of(&current_uri(view));
        if origin.starts_with("http") {
            let rule = app_js.browser_state.borrow().permission_for(&origin, "javascript");
            if let Some(settings) = webkit6::prelude::WebViewExt::settings(view) {
                settings.set_enable_javascript(rule != "block");
            }
        }
    });

    // Non-displayable responses (xpi, crx, binaries) become downloads.
    web_view.connect_decide_policy(|_, decision, decision_type| {
        if decision_type == webkit6::PolicyDecisionType::Response
            && let Some(response) = decision.downcast_ref::<webkit6::ResponsePolicyDecision>()
            && !response.is_mime_type_supported()
        {
            decision.download();
            return true;
        }
        false
    });

    // window.open / target=_blank -> new tab, unless pop-ups are blocked for the site.
    let app_create = app.clone();
    web_view.connect_create(move |view, action| {
        let origin = origin_of(&current_uri(view));
        if app_create.browser_state.borrow().permission_for(&origin, "popups") == "block"
            && !action.is_user_gesture()
        {
            return None;
        }
        if let Some(uri) = action.request().and_then(|request| request.uri()) {
            let tab = create_tab(&app_create, &uri);
            if let Some(index) = app_create.notebook.page_num(&tab.web_view) {
                app_create.notebook.set_current_page(Some(index));
                scroll_tab_strip_to_end(&app_create);
            }
        }
        None
    });

    let tab_audio = tab.clone();
    web_view.connect_is_playing_audio_notify(move |_| update_tab_audio(&tab_audio));

    let tab_muted = tab.clone();
    web_view.connect_is_muted_notify(move |_| update_tab_audio(&tab_muted));

    let app_load = app.clone();
    let tab_load = tab.clone();
    web_view.connect_load_changed(move |view, event| {
        tab_load
            .loading
            .set(matches!(event, webkit6::LoadEvent::Started | LoadEvent::Redirected | LoadEvent::Committed));
        if event == LoadEvent::Finished {
            let uri = current_uri(view);
            let title = current_title(view);
            view.set_zoom_level(app_load.browser_state.borrow().zoom_for_uri(&uri));
            if uri != app_load.new_tab_uri && is_internal_uri(&app_load, &uri) {
                render_internal_page(&app_load, &tab_load);
            } else if !uri.is_empty() && uri != app_load.new_tab_uri {
                app_load.browser_state.borrow_mut().add_history(
                    if title.is_empty() { &uri } else { &title },
                    &uri,
                    Utc::now().timestamp(),
                );
                refresh_internal_pages(&app_load);
                autofill_credentials(&app_load, view);
            }
        }
        if let Some(current) = current_tab(&app_load) && current.web_view == tab_load.web_view {
            sync_window(&app_load, &tab_load);
        }
    });

    let app_script = app.clone();
    let script_view = web_view.clone();
    manager.connect_script_message_received(Some("ubar"), move |_, result| {
        if result.is_string() {
            let message = result.to_string();
            handle_script_message(&app_script, &script_view, &message);
        }
    });

    update_tab_favicon(&tab);
    update_tab_audio(&tab);
    update_tab_title(app, &tab);
    apply_site_user_agent(&web_view, uri);
    web_view.load_uri(uri);
    glib::idle_add_local_once(move || animate_tab_opacity(&tab_box, 0.0, 1.0));
    tab
}

// Strips the page down to its main article with reading controls.
// Running it again reloads the page (toggle off).
const READER_JS: &str = r#"(function(){
if(document.getElementById('ubar-reader-style')){location.reload();return;}
var art=document.querySelector('article');
if(!art){var best=null,score=0;
document.querySelectorAll('main,section,div').forEach(function(el){
var s=0,ps=el.querySelectorAll('p');
for(var i=0;i<ps.length;i++)s+=ps[i].innerText.length;
if(s>score){score=s;best=el;}});
art=best||document.body;}
var title=document.title,content=art.innerHTML;
document.body.innerHTML=
'<div id="ubar-reader">'+
'<div class="ubar-reader-bar">'+
'<button data-act="smaller">A-</button><button data-act="larger">A+</button>'+
'<button data-theme="light">Light</button><button data-theme="sepia">Sepia</button><button data-theme="dark">Dark</button>'+
'<button data-act="close">Exit reader</button>'+
'</div><h1>'+title+'</h1><div class="ubar-reader-content">'+content+'</div></div>';
var st=document.createElement('style');st.id='ubar-reader-style';
st.textContent='body{margin:0;background:var(--rb,#fff);color:var(--rc,#1a1a1a);}'+
'#ubar-reader{max-width:44em;margin:0 auto;padding:24px;font:var(--rs,19px)/1.7 Georgia,serif;}'+
'#ubar-reader img{max-width:100%;height:auto;}'+
'#ubar-reader .ubar-reader-content *{background:transparent!important;color:inherit!important;float:none!important;width:auto!important;max-width:100%!important;}'+
'#ubar-reader aside,#ubar-reader nav,#ubar-reader iframe,#ubar-reader form,#ubar-reader button:not(.ubar-reader-bar button){display:none;}'+
'.ubar-reader-bar{position:sticky;top:0;display:flex;gap:8px;padding:10px 0;background:inherit;font:14px system-ui;}'+
'.ubar-reader-bar button{display:inline-block!important;padding:6px 12px;border-radius:999px;border:1px solid rgba(128,128,128,.4);background:transparent;color:inherit;cursor:pointer;}';
document.head.appendChild(st);
var size=19;
document.querySelector('.ubar-reader-bar').addEventListener('click',function(e){
var b=e.target.closest('button');if(!b)return;
if(b.dataset.act==='close'){location.reload();return;}
if(b.dataset.act==='smaller')size=Math.max(12,size-2);
if(b.dataset.act==='larger')size=Math.min(36,size+2);
document.documentElement.style.setProperty('--rs',size+'px');
if(b.dataset.theme==='light'){document.documentElement.style.setProperty('--rb','#fff');document.documentElement.style.setProperty('--rc','#1a1a1a');}
if(b.dataset.theme==='sepia'){document.documentElement.style.setProperty('--rb','#f4ecd8');document.documentElement.style.setProperty('--rc','#5b4636');}
if(b.dataset.theme==='dark'){document.documentElement.style.setProperty('--rb','#1e1e1e');document.documentElement.style.setProperty('--rc','#d4d4d4');}
});
window.scrollTo(0,0);
})();"#;

fn toggle_reading_mode(app: &Rc<AppState>) {
    if let Some(tab) = current_tab(app) {
        let uri = current_uri(&tab.web_view);
        if uri.starts_with("http") {
            evaluate_js(&tab.web_view, READER_JS, "ubar://reader");
        }
    }
}

// Private window: ephemeral session, nothing written to disk, no history.
fn open_incognito_window(app: &Rc<AppState>) {
    let session = NetworkSession::new_ephemeral();
    let web_view: WebView = glib::Object::builder()
        .property("network-session", &session)
        .build();
    configure_web_view(&web_view, &app.browser_state.borrow().settings);

    let entry = Entry::new();
    entry.add_css_class("ubar-address");
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some("Incognito — nothing is saved"));

    let header = HeaderBar::new();
    header.set_title_widget(Some(&entry));

    let window = gtk4::Window::builder()
        .default_width(1000)
        .default_height(700)
        .title("ubar (Incognito)")
        .build();
    if let Some(application) = app.window.application() {
        window.set_application(Some(&application));
    }
    window.set_titlebar(Some(&header));
    web_view.set_hexpand(true);
    web_view.set_vexpand(true);
    window.set_child(Some(&web_view));

    let engine = app.browser_state.borrow().settings.search_engine.clone();
    let view_entry = web_view.clone();
    entry.connect_activate(move |entry| {
        if let Some(uri) = normalize_uri(&entry.text(), &engine) {
            view_entry.load_uri(&uri);
        }
    });
    let entry_sync = entry.clone();
    web_view.connect_uri_notify(move |view| {
        entry_sync.set_text(&current_uri(view));
    });

    window.present();
    entry.grab_focus();
}

fn open_task_manager(app: &Rc<AppState>) {
    let list = GtkBox::new(Orientation::Vertical, 6);
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);

    let note = Label::new(Some(
        "Each tab runs in its own WebKit process. WebKitGTK does not expose per-process \
         memory to the embedder; kill a tab here to reclaim its resources.",
    ));
    note.set_wrap(true);
    note.set_xalign(0.0);
    list.append(&note);

    let window = gtk4::Window::builder()
        .default_width(520)
        .default_height(420)
        .title("Task Manager")
        .transient_for(&app.window)
        .build();

    for index in 0..app.notebook.n_pages() {
        if let Some(page) = app.notebook.nth_page(Some(index))
            && let Some(tab) = unsafe { page.data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
        {
            let row = GtkBox::new(Orientation::Horizontal, 8);
            let title = current_title(&tab.web_view);
            let uri = current_uri(&tab.web_view);
            let label = Label::new(Some(if title.is_empty() { &uri } else { &title }));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_hexpand(true);
            label.set_xalign(0.0);
            let kill = Button::with_label("Kill");
            let app_kill = app.clone();
            let tab_kill = tab.clone();
            let window_kill = window.clone();
            kill.connect_clicked(move |_| {
                close_tab(&app_kill, &tab_kill);
                window_kill.close();
            });
            row.append(&label);
            row.append(&kill);
            list.append(&row);
        }
    }

    let scroller = ScrolledWindow::new();
    scroller.set_child(Some(&list));
    window.set_child(Some(&scroller));
    window.present();
}

fn build_menu(app: &Rc<AppState>) {
    let menu_box = GtkBox::new(Orientation::Vertical, 4);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);

    let home = Button::with_label("Home");
    let incognito = Button::with_label("New Incognito Window");
    let history = Button::with_label("History");
    let bookmarks = Button::with_label("Bookmarks");
    let downloads = Button::with_label("Downloads");
    let reader = Button::with_label("Reading Mode");
    let task_manager = Button::with_label("Task Manager");
    let extensions = Button::with_label("Extensions");
    let settings = Button::with_label("Settings");
    menu_box.append(&home);
    menu_box.append(&incognito);
    menu_box.append(&history);
    menu_box.append(&bookmarks);
    menu_box.append(&downloads);
    menu_box.append(&reader);
    menu_box.append(&task_manager);
    menu_box.append(&extensions);
    menu_box.append(&settings);

    let popover = Popover::new();
    popover.set_child(Some(&menu_box));
    app.menu_button.set_popover(Some(&popover));

    let app_home = app.clone();
    home.connect_clicked(move |_| {
        let homepage = app_home.browser_state.borrow().settings.homepage_uri.clone();
        if homepage.is_empty() {
            load_uri_in_current_tab(&app_home, &app_home.new_tab_uri);
        } else {
            load_uri_in_current_tab(&app_home, &homepage);
        }
    });

    let app_history = app.clone();
    history.connect_clicked(move |_| load_uri_in_current_tab(&app_history, &app_history.history_uri));

    let app_bookmarks = app.clone();
    bookmarks.connect_clicked(move |_| load_uri_in_current_tab(&app_bookmarks, &app_bookmarks.bookmarks_uri));

    let app_downloads = app.clone();
    downloads.connect_clicked(move |_| {
        load_uri_in_current_tab(&app_downloads, &app_downloads.downloads_uri)
    });

    let app_incognito = app.clone();
    incognito.connect_clicked(move |_| open_incognito_window(&app_incognito));

    let app_reader = app.clone();
    reader.connect_clicked(move |_| toggle_reading_mode(&app_reader));

    let app_tasks = app.clone();
    task_manager.connect_clicked(move |_| open_task_manager(&app_tasks));

    let app_extensions = app.clone();
    extensions.connect_clicked(move |_| {
        load_uri_in_current_tab(&app_extensions, &app_extensions.extensions_uri)
    });

    let app_settings = app.clone();
    settings.connect_clicked(move |_| load_uri_in_current_tab(&app_settings, &app_settings.settings_uri));
}

pub fn run() {
    let application = Application::builder().application_id(APP_ID).build();
    application.connect_activate(|gtk_app| {
        // Must precede any WebView/NetworkSession so it reaches every web process.
        tune_memory_for_host();

        let css = gtk4::CssProvider::new();
        css.load_from_data(
            "
            .ubar-tab {
                min-height: 0;
                padding: 0 2px 0 10px;
                border-radius: 11px;
                background: alpha(currentColor, 0.03);
                box-shadow: inset 0 0 0 1px alpha(currentColor, 0.06);
                transition: 140ms ease;
            }

            .ubar-tab-active {
                background: alpha(@accent_color, 0.14);
                box-shadow: inset 0 0 0 1px alpha(@accent_color, 0.32);
            }

            .ubar-tab-title {
                font-weight: 450;
            }

            .ubar-tab-active .ubar-tab-title {
                font-weight: 700;
            }

            .ubar-tab-close {
                min-width: 22px;
                min-height: 22px;
                padding: 0;
            }

            .ubar-tab-audio {
                min-width: 14px;
                min-height: 14px;
            }

            .ubar-toolbar {
                background: alpha(currentColor, 0.02);
            }

            .ubar-toolbar button {
                border-radius: 999px;
                min-width: 30px;
                min-height: 30px;
                padding: 4px;
            }

            .ubar-address {
                border-radius: 999px;
                padding: 0 14px;
                background: alpha(currentColor, 0.06);
                border: none;
                box-shadow: none;
            }

            .ubar-address:focus {
                background: alpha(@accent_color, 0.08);
                box-shadow: 0 0 0 2px alpha(@accent_color, 0.4);
            }

            .ubar-find-bar {
                padding: 6px;
                border-radius: 10px;
                background: alpha(currentColor, 0.05);
            }

            .ubar-find-bar entry {
                border-radius: 999px;
            }
            ",
        );
        if let Some(display) = gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &css,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let window = ApplicationWindow::builder()
            .application(gtk_app)
            .default_width(1200)
            .default_height(800)
            .title("ubar")
            .build();

        let back_button = Button::from_icon_name("go-previous-symbolic");
        let forward_button = Button::from_icon_name("go-next-symbolic");
        let reload_button = Button::from_icon_name("view-refresh-symbolic");
        let address_entry = Entry::new();
        let bookmark_button = Button::from_icon_name("bookmark-new-symbolic");
        let extensions_button = Button::from_icon_name("application-x-addon-symbolic");
        extensions_button.set_tooltip_text(Some("Extensions"));
        let new_tab_button = Button::from_icon_name("list-add-symbolic");
        let menu_button = MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");
        let window_controls = WindowControls::new(PackType::End);

        let notebook = Notebook::new();
        notebook.set_show_tabs(false);

        let tab_bar = GtkBox::new(Orientation::Horizontal, 0);
        tab_bar.set_halign(Align::Start);
        let tab_scroller = ScrolledWindow::new();
        tab_scroller.set_hexpand(true);
        tab_scroller.set_vexpand(false);
        tab_scroller.set_hscrollbar_policy(gtk4::PolicyType::Automatic);
        tab_scroller.set_vscrollbar_policy(gtk4::PolicyType::Never);
        tab_scroller.set_child(Some(&tab_bar));
        let wheel_scroll = EventControllerScroll::new(EventControllerScrollFlags::VERTICAL);
        let tab_scroller_ref = tab_scroller.clone();
        wheel_scroll.connect_scroll(move |_, _, dy| {
            let adj = tab_scroller_ref.hadjustment();
            let step = if dy.abs() < 0.01 { 0.0 } else { dy * 56.0 };
            let target = (adj.value() + step).clamp(adj.lower(), (adj.upper() - adj.page_size()).max(adj.lower()));
            adj.set_value(target);
            glib::Propagation::Stop
        });
        tab_scroller.add_controller(wheel_scroll);

        let tabs_row = GtkBox::new(Orientation::Horizontal, 6);
        tabs_row.set_hexpand(true);
        tabs_row.set_margin_start(6);
        tabs_row.set_margin_end(6);
        tabs_row.set_margin_top(0);
        tabs_row.set_margin_bottom(0);
        tabs_row.append(&tab_scroller);

        let end_controls = GtkBox::new(Orientation::Horizontal, 6);
        end_controls.append(&new_tab_button);
        end_controls.append(&window_controls);

        let header_row = GtkBox::new(Orientation::Horizontal, 8);
        header_row.set_hexpand(true);
        header_row.append(&tabs_row);
        header_row.append(&end_controls);

        let header_bar = HeaderBar::new();
        header_bar.set_show_title_buttons(false);
        header_bar.set_title_widget(Some(&header_row));

        let toolbar = GtkBox::new(Orientation::Horizontal, 6);
        toolbar.add_css_class("ubar-toolbar");
        address_entry.add_css_class("ubar-address");
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);
        toolbar.set_margin_start(6);
        toolbar.set_margin_end(6);
        address_entry.set_hexpand(true);
        address_entry.set_height_request(TAB_HEIGHT);
        bookmark_button.set_has_frame(false);
        extensions_button.set_has_frame(false);
        new_tab_button.set_has_frame(false);
        toolbar.append(&back_button);
        toolbar.append(&forward_button);
        toolbar.append(&reload_button);
        toolbar.append(&address_entry);
        toolbar.append(&bookmark_button);
        toolbar.append(&extensions_button);
        toolbar.append(&menu_button);

        // Find-in-page bar (hidden until Ctrl+F).
        let find_entry = Entry::new();
        find_entry.set_placeholder_text(Some("Find in page"));
        find_entry.set_width_request(280);
        let find_prev = Button::from_icon_name("go-up-symbolic");
        let find_next = Button::from_icon_name("go-down-symbolic");
        let find_close = Button::from_icon_name("window-close-symbolic");
        for button in [&find_prev, &find_next, &find_close] {
            button.set_has_frame(false);
        }
        let find_bar = GtkBox::new(Orientation::Horizontal, 6);
        find_bar.add_css_class("ubar-find-bar");
        find_bar.set_halign(Align::End);
        find_bar.set_margin_start(6);
        find_bar.set_margin_end(6);
        find_bar.set_margin_bottom(6);
        find_bar.append(&find_entry);
        find_bar.append(&find_prev);
        find_bar.append(&find_next);
        find_bar.append(&find_close);
        find_bar.set_visible(false);

        let vbox = GtkBox::new(Orientation::Vertical, 0);
        vbox.append(&toolbar);
        vbox.append(&find_bar);
        vbox.append(&notebook);
        window.set_titlebar(Some(&header_bar));
        window.set_child(Some(&vbox));

        // Persistent session: cookies, cache, favicons, downloads.
        let data_root = dirs::data_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("ubar");
        let cache_root = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("ubar");
        let network_session = NetworkSession::new(
            data_root.join("web-data").to_str(),
            cache_root.to_str(),
        );
        if let Some(cookies) = network_session.cookie_manager() {
            cookies.set_persistent_storage(
                &data_root.join("cookies.sqlite").to_string_lossy(),
                CookiePersistentStorage::Sqlite,
            );
        }
        if let Some(data_manager) = network_session.website_data_manager() {
            data_manager.set_favicons_enabled(true);
        }
        let app = Rc::new(AppState {
            window,
            notebook,
            tab_bar,
            tab_scroller,
            back_button,
            forward_button,
            reload_button,
            address_entry,
            bookmark_button,
            new_tab_button,
            menu_button,
            find_bar,
            find_entry,
            network_session,
            extensions: RefCell::new(crate::extensions::load()),
            closed_tabs: RefCell::new(Vec::new()),
            browser_state: RefCell::new(BrowserState::load()),
            vault: RefCell::new(Vault::load()),
            active_downloads: RefCell::new(Vec::new()),
            new_tab_uri: asset_uri("assets/newtab/index.html"),
            history_uri: asset_uri("assets/pages/history/index.html"),
            bookmarks_uri: asset_uri("assets/pages/bookmarks/index.html"),
            settings_uri: asset_uri("assets/pages/settings/index.html"),
            extensions_uri: asset_uri("assets/pages/extensions/index.html"),
            downloads_uri: asset_uri("assets/pages/downloads/index.html"),
        });

        // Interrupted downloads from a previous run can never resume.
        {
            let mut state = app.browser_state.borrow_mut();
            for entry in &mut state.downloads {
                if entry.status == "active" {
                    entry.status = "failed".into();
                }
            }
            state.save();
        }
        apply_theme(&app.browser_state.borrow().settings.theme);
        apply_cookie_policy(&app);

        build_menu(&app);

        // Downloads: .xpi/.crx go to a staging dir and install as extensions,
        // everything else lands in the configured download dir with live tracking.
        let pending_dir = cache_root.join("pending");
        let app_download = app.clone();
        app.network_session.connect_download_started(move |_, download| {
            let pending = pending_dir.clone();
            let entry_id = Rc::new(Cell::new(0u64));

            let app_decide = app_download.clone();
            let entry_id_decide = entry_id.clone();
            download.connect_decide_destination(move |download, suggested| {
                let name = if suggested.is_empty() { "download" } else { suggested };
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".xpi") || lower.ends_with(".crx")
                    || download
                        .request()
                        .and_then(|r| r.uri())
                        .is_some_and(|u| u.contains("service/update2/crx"))
                {
                    let _ = std::fs::create_dir_all(&pending);
                    let staged = if lower.ends_with(".crx") || lower.ends_with(".xpi") {
                        pending.join(name)
                    } else {
                        pending.join(format!("{name}.crx"))
                    };
                    download.set_allow_overwrite(true);
                    download.set_destination(&staged.to_string_lossy());
                    return true;
                }

                let dir = download_directory(&app_decide);
                let mut path = dir.join(name);
                let mut counter = 1;
                while path.exists() {
                    path = dir.join(format!("{counter}-{name}"));
                    counter += 1;
                }
                download.set_destination(&path.to_string_lossy());

                let uri = download
                    .request()
                    .and_then(|request| request.uri())
                    .map(|uri| uri.to_string())
                    .unwrap_or_default();
                let id = app_decide.browser_state.borrow_mut().add_download(
                    &uri,
                    &path.to_string_lossy(),
                    name,
                    Utc::now().timestamp(),
                );
                entry_id_decide.set(id);
                app_decide
                    .active_downloads
                    .borrow_mut()
                    .push((id, download.clone()));
                refresh_downloads_pages(&app_decide);

                // Live progress: poll twice a second while the transfer runs.
                let app_tick = app_decide.clone();
                let download_tick = download.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
                    let mut active = false;
                    {
                        let mut state = app_tick.browser_state.borrow_mut();
                        if let Some(entry) = state.download_mut(id) {
                            if entry.status == "active" {
                                active = true;
                                entry.received = download_tick.received_data_length();
                                if let Some(response) = download_tick.response() {
                                    let length = response.content_length();
                                    if length > 0 {
                                        entry.total = length;
                                    }
                                }
                            }
                        }
                    }
                    refresh_downloads_pages(&app_tick);
                    if active {
                        glib::ControlFlow::Continue
                    } else {
                        glib::ControlFlow::Break
                    }
                });
                true
            });

            let app_failed = app_download.clone();
            let entry_id_failed = entry_id.clone();
            download.connect_failed(move |download, error| {
                let id = entry_id_failed.get();
                if id == 0 {
                    return;
                }
                let cancelled = error.matches(webkit6::DownloadError::CancelledByUser);
                {
                    let mut state = app_failed.browser_state.borrow_mut();
                    if let Some(entry) = state.download_mut(id) {
                        entry.status = if cancelled { "cancelled".into() } else { "failed".into() };
                        entry.received = download.received_data_length();
                    }
                    state.save();
                }
                app_failed
                    .active_downloads
                    .borrow_mut()
                    .retain(|(entry_id, _)| *entry_id != id);
                refresh_downloads_pages(&app_failed);
            });

            let app_finished = app_download.clone();
            let entry_id_finished = entry_id.clone();
            download.connect_finished(move |download| {
                let id = entry_id_finished.get();
                if id != 0 {
                    {
                        let mut state = app_finished.browser_state.borrow_mut();
                        if let Some(entry) = state.download_mut(id) {
                            if entry.status == "active" {
                                entry.status = "done".into();
                                entry.received = download.received_data_length();
                                if entry.total == 0 {
                                    entry.total = entry.received;
                                }
                            }
                        }
                        state.save();
                    }
                    app_finished
                        .active_downloads
                        .borrow_mut()
                        .retain(|(entry_id, _)| *entry_id != id);
                    refresh_downloads_pages(&app_finished);
                    return;
                }

                let Some(dest) = download.destination() else {
                    return;
                };
                let lower = dest.to_ascii_lowercase();
                if !lower.ends_with(".xpi") && !lower.ends_with(".crx") {
                    return;
                }
                let path = std::path::PathBuf::from(dest.as_str());
                match crate::extensions::install_file(&path) {
                    Ok(name) => {
                        println!("ubar: installed extension: {name}");
                        *app_finished.extensions.borrow_mut() = crate::extensions::load();
                        refresh_internal_pages(&app_finished);
                        load_uri_in_current_tab(&app_finished, &app_finished.extensions_uri);
                    }
                    Err(error) => eprintln!("ubar: extension install failed: {error}"),
                }
                let _ = std::fs::remove_file(&path);
            });
        });

        let app_find_changed = app.clone();
        app.find_entry.connect_changed(move |_| find_search(&app_find_changed));
        let app_find_activate = app.clone();
        app.find_entry.connect_activate(move |_| {
            if let Some(tab) = current_tab(&app_find_activate)
                && let Some(finder) = tab.web_view.find_controller()
            {
                finder.search_next();
            }
        });
        let app_find_prev = app.clone();
        find_prev.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_find_prev)
                && let Some(finder) = tab.web_view.find_controller()
            {
                finder.search_previous();
            }
        });
        let app_find_next = app.clone();
        find_next.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_find_next)
                && let Some(finder) = tab.web_view.find_controller()
            {
                finder.search_next();
            }
        });
        let app_find_close = app.clone();
        find_close.connect_clicked(move |_| close_find_bar(&app_find_close));

        // Save open tabs for session restore.
        let app_quit = app.clone();
        app.window.connect_close_request(move |_| {
            let mut uris = Vec::new();
            for index in 0..app_quit.notebook.n_pages() {
                if let Some(page) = app_quit.notebook.nth_page(Some(index))
                    && let Some(tab) = unsafe { page.data::<TabState>("tab-state") }
                        .map(|ptr| unsafe { ptr.as_ref().clone() })
                {
                    let uri = current_uri(&tab.web_view);
                    if !uri.is_empty() {
                        uris.push(uri);
                    }
                }
            }
            let mut state = app_quit.browser_state.borrow_mut();
            state.open_tabs = uris;
            state.save();
            glib::Propagation::Proceed
        });

        let app_back = app.clone();
        app.back_button.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_back) {
                tab.web_view.go_back();
            }
        });

        let app_forward = app.clone();
        app.forward_button.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_forward) {
                tab.web_view.go_forward();
            }
        });

        let app_reload = app.clone();
        app.reload_button.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_reload) {
                if tab.loading.get() {
                    tab.web_view.stop_loading();
                } else {
                    tab.web_view.reload();
                }
            }
        });

        let app_entry = app.clone();
        app.address_entry.connect_activate(move |entry| {
            let engine = app_entry.browser_state.borrow().settings.search_engine.clone();
            if let Some(uri) = normalize_uri(&entry.text(), &engine) {
                load_uri_in_current_tab(&app_entry, &uri);
            }
        });

        let app_bookmark = app.clone();
        app.bookmark_button.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_bookmark) {
                let uri = current_uri(&tab.web_view);
                if uri.is_empty() || is_internal_uri(&app_bookmark, &uri) {
                    return;
                }

                let title = current_title(&tab.web_view);
                app_bookmark.browser_state.borrow_mut().toggle_bookmark(
                    if title.is_empty() { &uri } else { &title },
                    &uri,
                    Utc::now().timestamp(),
                );
                update_bookmark_button(&app_bookmark);
                refresh_internal_pages(&app_bookmark);
            }
        });

        let app_ext_button = app.clone();
        extensions_button.connect_clicked(move |_| {
            load_uri_in_current_tab(&app_ext_button, &app_ext_button.extensions_uri)
        });

        let app_new = app.clone();
        app.new_tab_button.connect_clicked(move |_| {
            let tab = create_tab(&app_new, &app_new.new_tab_uri);
            if let Some(index) = app_new.notebook.page_num(&tab.web_view) {
                app_new.notebook.set_current_page(Some(index));
                sync_window(&app_new, &tab);
                focus_address_bar(&app_new);
                scroll_tab_strip_to_end(&app_new);
            }
        });

        let app_switch = app.clone();
        app.notebook.connect_switch_page(move |notebook, page, page_num| {
            if let Some(tab) = unsafe { page
                .data::<TabState>("tab-state") }
                .map(|ptr| unsafe { ptr.as_ref().clone() })
            {
        let total = notebook.n_pages();
                let _ = total;
                update_active_tab_styles(notebook, page_num);
                sync_window(&app_switch, &tab);
                scroll_tab_into_view(&app_switch, &tab.tab_box);
            }
        });

        let controller = EventControllerKey::new();
        let app_keys = app.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            if !state.contains(gdk::ModifierType::CONTROL_MASK) {
                if key == gdk::Key::Escape && state.contains(gdk::ModifierType::SHIFT_MASK) {
                    open_task_manager(&app_keys);
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::F9 {
                    toggle_reading_mode(&app_keys);
                    return glib::Propagation::Stop;
                }
                if key == gdk::Key::Escape {
                    if app_keys.find_bar.is_visible() {
                        close_find_bar(&app_keys);
                        return glib::Propagation::Stop;
                    }
                    if let Some(tab) = current_tab(&app_keys) && tab.loading.get() {
                        tab.web_view.stop_loading();
                        return glib::Propagation::Stop;
                    }
                }
                if key == gdk::Key::F12 {
                    if let Some(tab) = current_tab(&app_keys)
                        && let Some(inspector) = tab.web_view.inspector()
                    {
                        inspector.show();
                    }
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }

            if key == gdk::Key::Tab {
                let total = app_keys.notebook.n_pages();
                if total > 0 {
                    let current = app_keys.notebook.current_page().unwrap_or(0);
                    let next = if state.contains(gdk::ModifierType::SHIFT_MASK) {
                        if current == 0 { total - 1 } else { current - 1 }
                    } else {
                        (current + 1) % total
                    };
                    select_tab(&app_keys, next);
                }
                return glib::Propagation::Stop;
            }

            match key {
                gdk::Key::t | gdk::Key::T => {
                    if state.contains(gdk::ModifierType::SHIFT_MASK) {
                        reopen_closed_tab(&app_keys);
                        return glib::Propagation::Stop;
                    }
                    let tab = create_tab(&app_keys, &app_keys.new_tab_uri);
                    if let Some(index) = app_keys.notebook.page_num(&tab.web_view) {
                        app_keys.notebook.set_current_page(Some(index));
                        sync_window(&app_keys, &tab);
                        focus_address_bar(&app_keys);
                        scroll_tab_strip_to_end(&app_keys);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::l | gdk::Key::L => {
                    focus_address_bar(&app_keys);
                    glib::Propagation::Stop
                }
                gdk::Key::n | gdk::Key::N => {
                    if state.contains(gdk::ModifierType::SHIFT_MASK) {
                        open_incognito_window(&app_keys);
                        return glib::Propagation::Stop;
                    }
                    glib::Propagation::Proceed
                }
                gdk::Key::j | gdk::Key::J => {
                    load_uri_in_current_tab(&app_keys, &app_keys.downloads_uri);
                    glib::Propagation::Stop
                }
                gdk::Key::r | gdk::Key::R => {
                    if let Some(tab) = current_tab(&app_keys) {
                        if tab.loading.get() {
                            tab.web_view.stop_loading();
                        } else {
                            tab.web_view.reload();
                        }
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::w | gdk::Key::W => {
                    if let Some(tab) = current_tab(&app_keys) {
                        close_tab(&app_keys, &tab);
                    }
                    glib::Propagation::Stop
                }
                gdk::Key::d | gdk::Key::D => {
                    app_keys.bookmark_button.emit_clicked();
                    glib::Propagation::Stop
                }
                gdk::Key::h | gdk::Key::H => {
                    load_uri_in_current_tab(&app_keys, &app_keys.history_uri);
                    glib::Propagation::Stop
                }
                gdk::Key::f | gdk::Key::F => {
                    open_find_bar(&app_keys);
                    glib::Propagation::Stop
                }
                gdk::Key::plus | gdk::Key::equal => {
                    zoom_current_tab(&app_keys, 0.1);
                    glib::Propagation::Stop
                }
                gdk::Key::minus => {
                    zoom_current_tab(&app_keys, -0.1);
                    glib::Propagation::Stop
                }
                gdk::Key::_0 => {
                    zoom_current_tab(&app_keys, 0.0);
                    glib::Propagation::Stop
                }
                gdk::Key::_1 => {
                    select_tab(&app_keys, 0);
                    glib::Propagation::Stop
                }
                gdk::Key::_2 => {
                    select_tab(&app_keys, 1);
                    glib::Propagation::Stop
                }
                gdk::Key::_3 => {
                    select_tab(&app_keys, 2);
                    glib::Propagation::Stop
                }
                gdk::Key::_4 => {
                    select_tab(&app_keys, 3);
                    glib::Propagation::Stop
                }
                gdk::Key::_5 => {
                    select_tab(&app_keys, 4);
                    glib::Propagation::Stop
                }
                gdk::Key::_6 => {
                    select_tab(&app_keys, 5);
                    glib::Propagation::Stop
                }
                gdk::Key::_7 => {
                    select_tab(&app_keys, 6);
                    glib::Propagation::Stop
                }
                gdk::Key::_8 => {
                    select_tab(&app_keys, 7);
                    glib::Propagation::Stop
                }
                gdk::Key::_9 => {
                    let total = app_keys.notebook.n_pages();
                    if total > 0 {
                        select_tab(&app_keys, total - 1);
                    }
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        app.window.add_controller(controller);

        // Restore last session, fall back to one fresh tab.
        let restore = app.browser_state.borrow().open_tabs.clone();
        if restore.is_empty() {
            create_tab(&app, &app.new_tab_uri);
        } else {
            for uri in &restore {
                create_tab(&app, uri);
            }
        }
        select_tab(&app, 0);

        unsafe {
            app.window.set_data("app-state", app.clone());
        }
        app.window.present();

        let app_start = app.clone();
        glib::idle_add_local_once(move || {
            if app_start.notebook.n_pages() > 0 {
                select_tab(&app_start, 0);
            }
        });
    });
    application.run();
}
