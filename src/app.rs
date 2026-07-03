use crate::state::{asset_uri, BrowserState};
use chrono::Utc;
use gtk4::gdk;
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, Button, DragSource, DropTarget, Entry,
    EventControllerKey, EventControllerScroll, EventControllerScrollFlags, GestureClick, Image,
    Label, MenuButton, Notebook, Orientation, Popover, ScrolledWindow,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use webkit6::prelude::*;
use webkit6::{LoadEvent, Settings, UserContentManager, WebView};

const APP_ID: &str = "dev.ghanti.ubar";
const TAB_WIDTH: i32 = 220;
const TAB_ANIMATION_MS: u32 = 140;

#[derive(Clone)]
struct TabState {
    web_view: WebView,
    tab_box: GtkBox,
    favicon: Image,
    title: Label,
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
    browser_state: RefCell<BrowserState>,
    new_tab_uri: String,
    history_uri: String,
    bookmarks_uri: String,
    settings_uri: String,
}

fn normalize_uri(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains("://") {
        return Some(trimmed.to_string());
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
    let page_num = app.notebook.current_page()?;
    let page = app.notebook.nth_page(Some(page_num))?;
    unsafe { page.data::<TabState>("tab-state") }
        .map(|ptr| unsafe { ptr.as_ref().clone() })
}

fn focus_address_bar(app: &AppState) {
    app.address_entry.grab_focus();
    app.address_entry.select_region(0, -1);
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

fn select_tab(app: &Rc<AppState>, index: u32) {
    if index >= app.notebook.n_pages() {
        return;
    }

    app.notebook.set_current_page(Some(index));
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
        if uri.is_empty() || uri == app.new_tab_uri || uri == app.history_uri || uri == app.bookmarks_uri || uri == app.settings_uri {
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

fn configure_web_view(web_view: &WebView) {
    let settings = Settings::new();
    settings.set_hardware_acceleration_policy(webkit6::HardwareAccelerationPolicy::Never);
    settings.set_enable_webgl(false);
    settings.set_enable_write_console_messages_to_stdout(true);
    settings.set_enable_developer_extras(true);
    web_view.set_settings(&settings);

    if let Some(session) = web_view.network_session() {
        if let Some(data_manager) = session.website_data_manager() {
            data_manager.set_favicons_enabled(true);
        }
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

fn build_settings_script(state: &BrowserState) -> String {
    format!(
        "window.ubarRenderSettings({{homepageUri:'{}',historyCount:{},bookmarkCount:{}}});",
        js_escape(&state.settings.homepage_uri),
        state.history.len(),
        state.bookmarks.len()
    )
}

fn render_internal_page(app: &Rc<AppState>, tab: &TabState) {
    let uri = current_uri(&tab.web_view);
    let state = app.browser_state.borrow();
    if uri == app.history_uri {
        evaluate_js(&tab.web_view, &build_history_script(&state), &app.history_uri);
    } else if uri == app.bookmarks_uri {
        evaluate_js(&tab.web_view, &build_bookmarks_script(&state), &app.bookmarks_uri);
    } else if uri == app.settings_uri {
        evaluate_js(&tab.web_view, &build_settings_script(&state), &app.settings_uri);
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
            if uri == app.history_uri || uri == app.bookmarks_uri || uri == app.settings_uri {
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

fn load_uri_in_current_tab(app: &Rc<AppState>, uri: &str) {
    if let Some(tab) = current_tab(app) {
        tab.web_view.load_uri(uri);
    }
}

fn handle_script_message(app: &Rc<AppState>, message: &str) {
    if let Some(uri) = message.strip_prefix("open:") {
        let decoded = glib::uri_unescape_string(uri, None::<&str>).unwrap_or_default();
        load_uri_in_current_tab(app, &decoded);
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

    if let Some(uri) = message.strip_prefix("save-homepage:") {
        let decoded = glib::uri_unescape_string(uri, None::<&str>).unwrap_or_default();
        app.browser_state.borrow_mut().settings.homepage_uri =
            normalize_uri(&decoded).unwrap_or_default();
        app.browser_state.borrow().save();
        refresh_internal_pages(app);
    }
}

fn create_tab(app: &Rc<AppState>, uri: &str) -> TabState {
    let manager = UserContentManager::new();
    let _ = manager.register_script_message_handler("ubar", None::<&str>);

    let web_view: WebView = glib::Object::builder()
        .property("user-content-manager", &manager)
        .build();
    configure_web_view(&web_view);

    let favicon = Image::from_icon_name("globe-symbolic");
    let title = Label::new(Some("New Tab"));
    title.add_css_class("ubar-tab-title");
    title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    title.set_max_width_chars(32);
    title.set_xalign(0.0);
    title.set_hexpand(true);

    let close_button = Button::from_icon_name("window-close-symbolic");
    close_button.set_has_frame(false);
    close_button.set_focusable(false);

    let tab_box = GtkBox::new(Orientation::Horizontal, 8);
    tab_box.add_css_class("ubar-tab");
    tab_box.set_margin_top(4);
    tab_box.set_margin_bottom(4);
    tab_box.set_margin_start(6);
    tab_box.set_margin_end(6);
    tab_box.set_size_request(TAB_WIDTH, -1);
    tab_box.set_opacity(0.0);
    tab_box.append(&favicon);
    tab_box.append(&title);
    tab_box.append(&close_button);

    web_view.set_hexpand(true);
    web_view.set_vexpand(true);
    app.notebook.append_page(&web_view, None::<&gtk4::Widget>);
    app.tab_bar.append(&tab_box);

    let tab = TabState {
        web_view: web_view.clone(),
        tab_box: tab_box.clone(),
        favicon: favicon.clone(),
        title: title.clone(),
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

    let app_load = app.clone();
    let tab_load = tab.clone();
    web_view.connect_load_changed(move |view, event| {
        tab_load
            .loading
            .set(matches!(event, webkit6::LoadEvent::Started | LoadEvent::Redirected | LoadEvent::Committed));
        if event == LoadEvent::Finished {
            let uri = current_uri(view);
            let title = current_title(view);
            if uri == app_load.history_uri || uri == app_load.bookmarks_uri || uri == app_load.settings_uri {
                render_internal_page(&app_load, &tab_load);
            } else if !uri.is_empty() && uri != app_load.new_tab_uri {
                app_load.browser_state.borrow_mut().add_history(
                    if title.is_empty() { &uri } else { &title },
                    &uri,
                    Utc::now().timestamp(),
                );
                refresh_internal_pages(&app_load);
            }
        }
        if let Some(current) = current_tab(&app_load) && current.web_view == tab_load.web_view {
            sync_window(&app_load, &tab_load);
        }
    });

    let app_script = app.clone();
    manager.connect_script_message_received(Some("ubar"), move |_, result| {
        if result.is_string() {
            let message = result.to_string();
            handle_script_message(&app_script, &message);
        }
    });

    update_tab_favicon(&tab);
    update_tab_title(app, &tab);
    web_view.load_uri(uri);
    glib::idle_add_local_once(move || animate_tab_opacity(&tab_box, 0.0, 1.0));
    tab
}

fn build_menu(app: &Rc<AppState>) {
    let menu_box = GtkBox::new(Orientation::Vertical, 4);
    menu_box.set_margin_top(8);
    menu_box.set_margin_bottom(8);
    menu_box.set_margin_start(8);
    menu_box.set_margin_end(8);

    let home = Button::with_label("Home");
    let history = Button::with_label("History");
    let bookmarks = Button::with_label("Bookmarks");
    let settings = Button::with_label("Settings");
    menu_box.append(&home);
    menu_box.append(&history);
    menu_box.append(&bookmarks);
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

    let app_settings = app.clone();
    settings.connect_clicked(move |_| load_uri_in_current_tab(&app_settings, &app_settings.settings_uri));
}

pub fn run() {
    let application = Application::builder().application_id(APP_ID).build();
    application.connect_activate(|gtk_app| {
        let css = gtk4::CssProvider::new();
        css.load_from_data(
            "
            .ubar-tab {
                padding: 3px 6px;
                border-radius: 12px;
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
        let new_tab_button = Button::from_icon_name("list-add-symbolic");
        let menu_button = MenuButton::new();
        menu_button.set_icon_name("open-menu-symbolic");

        let notebook = Notebook::new();
        notebook.set_show_tabs(false);

        let tab_bar = GtkBox::new(Orientation::Horizontal, 0);
        tab_bar.set_hexpand(true);
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
        tabs_row.set_margin_start(6);
        tabs_row.set_margin_end(6);
        tabs_row.append(&tab_scroller);
        tabs_row.append(&new_tab_button);

        let toolbar = GtkBox::new(Orientation::Horizontal, 6);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);
        toolbar.set_margin_start(6);
        toolbar.set_margin_end(6);
        address_entry.set_hexpand(true);
        bookmark_button.set_has_frame(false);
        new_tab_button.set_has_frame(false);
        toolbar.append(&back_button);
        toolbar.append(&forward_button);
        toolbar.append(&reload_button);
        toolbar.append(&address_entry);
        toolbar.append(&bookmark_button);
        toolbar.append(&menu_button);

        let vbox = GtkBox::new(Orientation::Vertical, 0);
        vbox.append(&toolbar);
        vbox.append(&tabs_row);
        vbox.append(&notebook);
        window.set_child(Some(&vbox));

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
            browser_state: RefCell::new(BrowserState::load()),
            new_tab_uri: asset_uri("assets/newtab/index.html"),
            history_uri: asset_uri("assets/pages/history/index.html"),
            bookmarks_uri: asset_uri("assets/pages/bookmarks/index.html"),
            settings_uri: asset_uri("assets/pages/settings/index.html"),
        });

        build_menu(&app);

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
            if let Some(uri) = normalize_uri(&entry.text()) {
                load_uri_in_current_tab(&app_entry, &uri);
            }
        });

        let app_bookmark = app.clone();
        app.bookmark_button.connect_clicked(move |_| {
            if let Some(tab) = current_tab(&app_bookmark) {
                let uri = current_uri(&tab.web_view);
                if uri.is_empty()
                    || uri == app_bookmark.new_tab_uri
                    || uri == app_bookmark.history_uri
                    || uri == app_bookmark.bookmarks_uri
                    || uri == app_bookmark.settings_uri
                {
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
                for index in 0..total {
                    if let Some(page) = notebook.nth_page(Some(index))
                        && let Some(other_tab) = unsafe { page
                            .data::<TabState>("tab-state") }
                            .map(|ptr| unsafe { ptr.as_ref().clone() })
                    {
                        other_tab.tab_box.remove_css_class("ubar-tab-active");
                        if index == page_num {
                            other_tab.tab_box.add_css_class("ubar-tab-active");
                        }
                    }
                }
                sync_window(&app_switch, &tab);
                scroll_tab_into_view(&app_switch, &tab.tab_box);
            }
        });

        let controller = EventControllerKey::new();
        let app_keys = app.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            if !state.contains(gdk::ModifierType::CONTROL_MASK) {
                if key == gdk::Key::Escape {
                    if let Some(tab) = current_tab(&app_keys) && tab.loading.get() {
                        tab.web_view.stop_loading();
                        return glib::Propagation::Stop;
                    }
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

        let first = create_tab(&app, &app.new_tab_uri);
        if let Some(index) = app.notebook.page_num(&first.web_view) {
            app.notebook.set_current_page(Some(index));
            sync_window(&app, &first);
        }

        unsafe {
            app.window.set_data("app-state", app.clone());
        }
        app.window.present();
    });
    application.run();
}
