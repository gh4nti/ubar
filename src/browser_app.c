#include "browser_app.h"
#include "browser_state.h"

#include <gtk/gtk.h>
#include <jsc/jsc.h>
#include <string.h>
#include <webkit/webkit.h>

typedef struct AppState AppState;
typedef struct TabState TabState;

static gboolean remove_tab_after_animation(gpointer user_data);
static void on_new_tab_clicked(GtkButton *button, gpointer user_data);
static TabState *get_tab_from_page(GtkWidget *page);
static TabState *get_current_tab(AppState *app);
static void load_new_tab_page(TabState *tab);

struct AppState {
    GtkWidget *window;
    GtkWidget *notebook;
    GtkWidget *toolbar;
    GtkWidget *tab_action_box;
    GtkWidget *window_controls;
    GtkWidget *back_button;
    GtkWidget *forward_button;
    GtkWidget *reload_button;
    GtkWidget *address_entry;
    GtkWidget *new_tab_button;
    GtkWidget *bookmark_button;
    GtkWidget *menu_button;
    GtkWidget *home_menu_item;
    GtkWidget *history_menu_item;
    GtkWidget *bookmarks_menu_item;
    GtkWidget *settings_menu_item;
    char *new_tab_uri;
    char *history_uri;
    char *bookmarks_uri;
    char *settings_uri;
    BrowserState *browser_state;
};

struct TabState {
    AppState *app;
    GtkWidget *page;
    GtkWidget *web_box;
    GtkWidget *tab_revealer;
    GtkWidget *tab_box;
    GtkWidget *favicon_image;
    GtkWidget *title_label;
    WebKitWebView *web_view;
    gboolean is_loading;
};

static char *
build_asset_uri(const char *relative_path)
{
    char *exe_path;
    char *exe_dir;
    char *asset_path;
    char *uri;

    exe_path = g_file_read_link("/proc/self/exe", NULL);
    if (exe_path == NULL) {
        return g_filename_to_uri(relative_path, NULL, NULL);
    }

    exe_dir = g_path_get_dirname(exe_path);
    asset_path = g_build_filename(exe_dir, relative_path, NULL);
    uri = g_filename_to_uri(asset_path, NULL, NULL);

    g_free(asset_path);
    g_free(exe_dir);
    g_free(exe_path);

    return uri;
}

static gboolean
tab_is_uri(TabState *tab, const char *uri)
{
    const char *current_uri;

    current_uri = webkit_web_view_get_uri(tab->web_view);
    return current_uri != NULL && g_strcmp0(current_uri, uri) == 0;
}

static gboolean
tab_is_new_tab(TabState *tab)
{
    return tab_is_uri(tab, tab->app->new_tab_uri);
}

static gboolean
tab_is_history_page(TabState *tab)
{
    return tab_is_uri(tab, tab->app->history_uri);
}

static gboolean
tab_is_bookmarks_page(TabState *tab)
{
    return tab_is_uri(tab, tab->app->bookmarks_uri);
}

static gboolean
tab_is_settings_page(TabState *tab)
{
    return tab_is_uri(tab, tab->app->settings_uri);
}

static gboolean
tab_is_internal_page(TabState *tab)
{
    return tab_is_new_tab(tab) || tab_is_history_page(tab) || tab_is_bookmarks_page(tab) ||
           tab_is_settings_page(tab);
}

static gboolean
tab_can_bookmark(TabState *tab)
{
    const char *uri;

    if (tab == NULL || tab_is_internal_page(tab)) {
        return FALSE;
    }

    uri = webkit_web_view_get_uri(tab->web_view);
    return uri != NULL && *uri != '\0';
}

static char *
format_timestamp(gint64 timestamp)
{
    GDateTime *dt;
    char *text;

    dt = g_date_time_new_from_unix_local(timestamp);
    if (dt == NULL) {
        return g_strdup("");
    }

    text = g_date_time_format(dt, "%a, %d %b %Y %H:%M");
    g_date_time_unref(dt);
    return text;
}

static void
append_js_string(GString *out, const char *value)
{
    const char *p;

    g_string_append_c(out, '\'');
    for (p = value != NULL ? value : ""; *p != '\0'; p++) {
        if (*p == '\\' || *p == '\'') {
            g_string_append_c(out, '\\');
        }
        if (*p == '\n' || *p == '\r') {
            g_string_append(out, "\\n");
            continue;
        }
        g_string_append_c(out, *p);
    }
    g_string_append_c(out, '\'');
}


static char *
origin_from_uri(const char *uri)
{
    GUri *parsed;
    const char *scheme;
    const char *host;
    gint port;
    char *origin;

    if (uri == NULL || *uri == '\0') {
        return NULL;
    }

    parsed = g_uri_parse(uri, G_URI_FLAGS_NONE, NULL);
    if (parsed == NULL) {
        return NULL;
    }

    scheme = g_uri_get_scheme(parsed);
    host = g_uri_get_host(parsed);
    port = g_uri_get_port(parsed);

    if (scheme == NULL || host == NULL) {
        g_uri_unref(parsed);
        return NULL;
    }

    if (port > 0) {
        origin = g_strdup_printf("%s://%s:%d", scheme, host, port);
    } else {
        origin = g_strdup_printf("%s://%s", scheme, host);
    }

    g_uri_unref(parsed);
    return origin;
}

static void
append_permission_value(GString *script, BrowserState *state, const char *key)
{
    g_string_append(script, key);
    g_string_append_c(script, ':');
    append_js_string(script, browser_state_get_default_permission(state, key));
}

static char *
build_history_script(BrowserState *state)
{
    GPtrArray *items;
    GString *script;
    guint i;

    items = browser_state_get_history(state);
    script = g_string_new("window.ubarRenderHistory({items:[");

    for (i = 0; i < items->len; i++) {
        BrowserHistoryEntry *entry;
        char *time_text;

        entry = g_ptr_array_index(items, i);
        time_text = format_timestamp(entry->timestamp);
        if (i > 0) {
            g_string_append_c(script, ',');
        }
        g_string_append(script, "{title:");
        append_js_string(script, entry->title);
        g_string_append(script, ",uri:");
        append_js_string(script, entry->uri);
        g_string_append(script, ",time:");
        append_js_string(script, time_text);
        g_string_append(script, "}");
        g_free(time_text);
    }

    g_string_append(script, "]});");
    return g_string_free(script, FALSE);
}

static char *
build_bookmarks_script(BrowserState *state)
{
    GPtrArray *items;
    GString *script;
    guint i;

    items = browser_state_get_bookmarks(state);
    script = g_string_new("window.ubarRenderBookmarks({items:[");

    for (i = 0; i < items->len; i++) {
        BrowserBookmarkEntry *entry;

        entry = g_ptr_array_index(items, i);
        if (i > 0) {
            g_string_append_c(script, ',');
        }
        g_string_append(script, "{title:");
        append_js_string(script, entry->title);
        g_string_append(script, ",uri:");
        append_js_string(script, entry->uri);
        g_string_append(script, "}");
    }

    g_string_append(script, "]});");
    return g_string_free(script, FALSE);
}

static char *
build_settings_script(BrowserState *state)
{
    GPtrArray *sites;
    GString *script;
    guint i;

    sites = browser_state_get_site_permissions(state);
    script = g_string_new("window.ubarRenderSettings({homepageUri:");
    append_js_string(script, browser_state_get_homepage_uri(state));
    g_string_append_printf(script,
                           ",historyCount:%u,bookmarkCount:%u",
                           browser_state_get_history(state)->len,
                           browser_state_get_bookmarks(state)->len);

    g_string_append(script, ",defaults:{");
    append_permission_value(script, state, "notifications");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "location");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "cookies");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "camera");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "microphone");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "javascript");
    g_string_append_c(script, ',');
    append_permission_value(script, state, "popups");
    g_string_append(script, "},sites:[");

    for (i = 0; i < sites->len; i++) {
        BrowserSitePermissionEntry *entry;

        entry = g_ptr_array_index(sites, i);
        if (i > 0) {
            g_string_append_c(script, ',');
        }
        g_string_append(script, "{origin:");
        append_js_string(script, entry->origin);
        g_string_append(script, ",notifications:");
        append_js_string(script, entry->notifications);
        g_string_append(script, ",location:");
        append_js_string(script, entry->location);
        g_string_append(script, ",cookies:");
        append_js_string(script, entry->cookies);
        g_string_append(script, ",camera:");
        append_js_string(script, entry->camera);
        g_string_append(script, ",microphone:");
        append_js_string(script, entry->microphone);
        g_string_append(script, ",javascript:");
        append_js_string(script, entry->javascript);
        g_string_append(script, ",popups:");
        append_js_string(script, entry->popups);
        g_string_append(script, "}");
    }

    g_string_append(script, "]});");
    return g_string_free(script, FALSE);
}

static void
evaluate_script(WebKitWebView *web_view, const char *script, const char *source_uri)
{
    webkit_web_view_evaluate_javascript(web_view, script, -1, NULL, source_uri, NULL, NULL, NULL);
}

static void
render_internal_page(TabState *tab)
{
    char *script;

    if (tab_is_history_page(tab)) {
        script = build_history_script(tab->app->browser_state);
        evaluate_script(tab->web_view, script, tab->app->history_uri);
        g_free(script);
        return;
    }

    if (tab_is_bookmarks_page(tab)) {
        script = build_bookmarks_script(tab->app->browser_state);
        evaluate_script(tab->web_view, script, tab->app->bookmarks_uri);
        g_free(script);
        return;
    }

    if (tab_is_settings_page(tab)) {
        script = build_settings_script(tab->app->browser_state);
        evaluate_script(tab->web_view, script, tab->app->settings_uri);
        g_free(script);
    }
}

static void
refresh_internal_pages(AppState *app)
{
    int page_count;
    int i;

    page_count = gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook));
    for (i = 0; i < page_count; i++) {
        GtkWidget *page;
        TabState *tab;

        page = gtk_notebook_get_nth_page(GTK_NOTEBOOK(app->notebook), i);
        tab = get_tab_from_page(page);
        if (tab != NULL && (tab_is_history_page(tab) || tab_is_bookmarks_page(tab) ||
                            tab_is_settings_page(tab))) {
            render_internal_page(tab);
        }
    }
}

static void
update_bookmark_button(AppState *app)
{
    TabState *tab;
    const char *uri;

    tab = get_current_tab(app);
    if (!tab_can_bookmark(tab)) {
        gtk_widget_set_sensitive(app->bookmark_button, FALSE);
        gtk_button_set_icon_name(GTK_BUTTON(app->bookmark_button), "bookmark-new-symbolic");
        return;
    }

    uri = webkit_web_view_get_uri(tab->web_view);
    gtk_widget_set_sensitive(app->bookmark_button, TRUE);
    gtk_button_set_icon_name(GTK_BUTTON(app->bookmark_button),
                             browser_state_is_bookmarked(app->browser_state, uri)
                                 ? "starred-symbolic"
                                 : "bookmark-new-symbolic");
}

static void
load_uri_in_current_tab(AppState *app, const char *uri)
{
    TabState *tab;

    tab = get_current_tab(app);
    if (tab == NULL) {
        return;
    }

    if (g_strcmp0(uri, app->new_tab_uri) == 0) {
        load_new_tab_page(tab);
        return;
    }

    webkit_web_view_load_uri(tab->web_view, uri);
}

static char *
normalize_uri(const char *input)
{
    if (input == NULL) {
        return NULL;
    }

    while (g_ascii_isspace(*input)) {
        input++;
    }

    if (*input == '\0') {
        return NULL;
    }

    if (strstr(input, "://") != NULL) {
        return g_strdup(input);
    }

    return g_strdup_printf("https://%s", input);
}

static TabState *
get_tab_from_page(GtkWidget *page)
{
    if (page == NULL) {
        return NULL;
    }

    return g_object_get_data(G_OBJECT(page), "tab-state");
}

static TabState *
get_current_tab(AppState *app)
{
    GtkWidget *page;
    int page_num;

    page_num = gtk_notebook_get_current_page(GTK_NOTEBOOK(app->notebook));
    if (page_num < 0) {
        return NULL;
    }

    page = gtk_notebook_get_nth_page(GTK_NOTEBOOK(app->notebook), page_num);
    return get_tab_from_page(page);
}

static void
free_app_state(GtkWidget *window, gpointer user_data)
{
    AppState *app;

    (void)window;
    app = user_data;
    g_free(app->new_tab_uri);
    g_free(app->history_uri);
    g_free(app->bookmarks_uri);
    g_free(app->settings_uri);
    browser_state_free(app->browser_state);
    g_free(app);
}

static void
close_tab(TabState *tab)
{
    AppState *app;
    int next_page_num;
    int page_num;

    if (tab == NULL) {
        return;
    }

    app = tab->app;
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), tab->page);
    if (page_num < 0) {
        return;
    }

    if (gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook)) == 1) {
        gtk_window_destroy(GTK_WINDOW(app->window));
        return;
    }

    next_page_num = page_num;
    if (page_num == gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook)) - 1) {
        next_page_num = page_num - 1;
    }

    gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), next_page_num);
    gtk_widget_set_sensitive(tab->tab_box, FALSE);
    gtk_revealer_set_reveal_child(GTK_REVEALER(tab->tab_revealer), FALSE);
    g_timeout_add(140, remove_tab_after_animation, tab);
}

static gboolean
remove_tab_after_animation(gpointer user_data)
{
    TabState *tab;
    AppState *app;
    int page_num;

    tab = user_data;
    app = tab->app;
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), tab->page);
    if (page_num >= 0) {
        gtk_notebook_remove_page(GTK_NOTEBOOK(app->notebook), page_num);
    }

    return G_SOURCE_REMOVE;
}

static gboolean
reveal_tab_after_create(gpointer user_data)
{
    gtk_revealer_set_reveal_child(GTK_REVEALER(user_data), TRUE);
    return G_SOURCE_REMOVE;
}

static void
update_tab_favicon(TabState *tab)
{
    GdkTexture *favicon;

    favicon = webkit_web_view_get_favicon(tab->web_view);
    if (favicon != NULL) {
        gtk_image_set_from_paintable(GTK_IMAGE(tab->favicon_image), GDK_PAINTABLE(favicon));
        return;
    }

    gtk_image_set_from_icon_name(GTK_IMAGE(tab->favicon_image), "globe-symbolic");
}

static void
update_tab_label(TabState *tab)
{
    const char *title;
    const char *uri;

    title = webkit_web_view_get_title(tab->web_view);
    if (title != NULL && *title != '\0') {
        gtk_label_set_text(GTK_LABEL(tab->title_label), title);
        return;
    }

    if (tab_is_new_tab(tab)) {
        gtk_label_set_text(GTK_LABEL(tab->title_label), "New Tab");
        return;
    }

    if (tab_is_history_page(tab)) {
        gtk_label_set_text(GTK_LABEL(tab->title_label), "History");
        return;
    }

    if (tab_is_bookmarks_page(tab)) {
        gtk_label_set_text(GTK_LABEL(tab->title_label), "Bookmarks");
        return;
    }

    if (tab_is_settings_page(tab)) {
        gtk_label_set_text(GTK_LABEL(tab->title_label), "Settings");
        return;
    }

    uri = webkit_web_view_get_uri(tab->web_view);
    if (uri != NULL && *uri != '\0') {
        gtk_label_set_text(GTK_LABEL(tab->title_label), uri);
        return;
    }

    gtk_label_set_text(GTK_LABEL(tab->title_label), "New Tab");
}

static void
sync_window_to_tab(TabState *tab)
{
    AppState *app;
    const char *title;
    const char *uri;

    if (tab == NULL) {
        return;
    }

    app = tab->app;
    uri = webkit_web_view_get_uri(tab->web_view);
    title = webkit_web_view_get_title(tab->web_view);

    if (tab_is_new_tab(tab)) {
        gtk_editable_set_text(GTK_EDITABLE(app->address_entry), "");
    } else {
        gtk_editable_set_text(GTK_EDITABLE(app->address_entry), uri != NULL ? uri : "");
    }
    gtk_widget_set_sensitive(app->back_button, webkit_web_view_can_go_back(tab->web_view));
    gtk_widget_set_sensitive(app->forward_button, webkit_web_view_can_go_forward(tab->web_view));
    gtk_button_set_icon_name(GTK_BUTTON(app->reload_button),
                             tab->is_loading
                                 ? "process-stop-symbolic"
                                 : "view-refresh-symbolic");
    gtk_window_set_title(GTK_WINDOW(app->window), title != NULL ? title : "ubar");
    update_bookmark_button(app);
}

static void
configure_web_view(WebKitWebView *web_view)
{
    WebKitNetworkSession *network_session;
    WebKitSettings *settings;
    WebKitWebsiteDataManager *data_manager;

    settings = webkit_web_view_get_settings(web_view);
    network_session = webkit_web_view_get_network_session(web_view);
    data_manager = webkit_network_session_get_website_data_manager(network_session);

    webkit_website_data_manager_set_favicons_enabled(data_manager, TRUE);
    webkit_settings_set_hardware_acceleration_policy(
        settings,
        WEBKIT_HARDWARE_ACCELERATION_POLICY_NEVER);
    webkit_settings_set_enable_webgl(settings, FALSE);
    webkit_settings_set_enable_2d_canvas_acceleration(settings, FALSE);
    webkit_settings_set_enable_write_console_messages_to_stdout(settings, TRUE);
    webkit_settings_set_enable_developer_extras(settings, TRUE);
}

static void
load_new_tab_page(TabState *tab)
{
    webkit_web_view_load_uri(tab->web_view, tab->app->new_tab_uri);
}

static void
switch_tab(AppState *app, int direction)
{
    int page_num;
    int page_count;

    page_count = gtk_notebook_get_n_pages(GTK_NOTEBOOK(app->notebook));
    if (page_count < 2) {
        return;
    }

    page_num = gtk_notebook_get_current_page(GTK_NOTEBOOK(app->notebook));
    page_num = (page_num + direction + page_count) % page_count;
    gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), page_num);
}

static void
load_address(AppState *app)
{
    const char *input;
    char *uri;
    TabState *tab;

    tab = get_current_tab(app);
    if (tab == NULL) {
        return;
    }

    input = gtk_editable_get_text(GTK_EDITABLE(app->address_entry));
    uri = normalize_uri(input);
    if (uri == NULL) {
        return;
    }

    webkit_web_view_load_uri(tab->web_view, uri);
    g_free(uri);
}

static void
on_back_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    TabState *tab;

    (void)button;
    app = user_data;
    tab = get_current_tab(app);
    if (tab != NULL) {
        webkit_web_view_go_back(tab->web_view);
    }
}

static void
on_forward_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    TabState *tab;

    (void)button;
    app = user_data;
    tab = get_current_tab(app);
    if (tab != NULL) {
        webkit_web_view_go_forward(tab->web_view);
    }
}

static void
on_reload_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    TabState *tab;

    (void)button;
    app = user_data;
    tab = get_current_tab(app);
    if (tab == NULL) {
        return;
    }

    if (tab->is_loading) {
        webkit_web_view_stop_loading(tab->web_view);
        return;
    }

    if (tab_is_new_tab(tab)) {
        load_new_tab_page(tab);
        return;
    }

    if (tab_is_history_page(tab) || tab_is_bookmarks_page(tab) || tab_is_settings_page(tab)) {
        webkit_web_view_load_uri(tab->web_view, webkit_web_view_get_uri(tab->web_view));
        return;
    }

    webkit_web_view_reload(tab->web_view);
}

static void
on_address_activate(GtkEntry *entry, gpointer user_data)
{
    (void)entry;
    load_address(user_data);
}

static void
on_uri_changed(WebKitWebView *web_view, GParamSpec *pspec, gpointer user_data)
{
    TabState *tab;

    (void)web_view;
    (void)pspec;
    tab = user_data;

    update_tab_label(tab);
    if (tab == get_current_tab(tab->app)) {
        sync_window_to_tab(tab);
    }
}

static void
on_title_changed(WebKitWebView *web_view, GParamSpec *pspec, gpointer user_data)
{
    TabState *tab;

    (void)web_view;
    (void)pspec;
    tab = user_data;

    update_tab_label(tab);
    if (tab == get_current_tab(tab->app)) {
        sync_window_to_tab(tab);
    }
}

static void
on_favicon_changed(WebKitWebView *web_view, GParamSpec *pspec, gpointer user_data)
{
    TabState *tab;

    (void)web_view;
    (void)pspec;
    tab = user_data;

    update_tab_favicon(tab);
}

static void
on_script_message_received(WebKitUserContentManager *manager,
                           JSCValue *value,
                           gpointer user_data)
{
    AppState *app;
    TabState *tab;
    char *message;
    char *decoded;
    char *normalized;
    const char *current_title;
    const char *current_uri;

    (void)manager;
    tab = user_data;
    app = tab->app;
    if (!jsc_value_is_string(value)) {
        return;
    }

    message = jsc_value_to_string(value);
    if (message == NULL) {
        return;
    }

    if (g_str_has_prefix(message, "open:")) {
        decoded = g_uri_unescape_string(message + 5, NULL);
        if (decoded != NULL) {
            load_uri_in_current_tab(app, decoded);
        }
        g_free(decoded);
    } else if (g_str_has_prefix(message, "delete-bookmark:")) {
        decoded = g_uri_unescape_string(message + 16, NULL);
        if (decoded != NULL && browser_state_remove_bookmark(app->browser_state, decoded)) {
            refresh_internal_pages(app);
            update_bookmark_button(app);
        }
        g_free(decoded);
    } else if (g_str_has_prefix(message, "save-homepage:")) {
        decoded = g_uri_unescape_string(message + 14, NULL);
        normalized = normalize_uri(decoded);
        browser_state_set_homepage_uri(app->browser_state, normalized != NULL ? normalized : "");
        refresh_internal_pages(app);
        g_free(normalized);
        g_free(decoded);
    } else if (g_str_has_prefix(message, "save-permission-default:")) {
        char **parts = g_strsplit(message, ":", 3);
        if (parts[1] != NULL && parts[2] != NULL) {
            browser_state_set_default_permission(app->browser_state, parts[1], parts[2]);
            refresh_internal_pages(app);
        }
        g_strfreev(parts);
    } else if (g_str_has_prefix(message, "save-site-permission:")) {
        char **parts = g_strsplit(message, ":", 4);
        if (parts[1] != NULL && parts[2] != NULL && parts[3] != NULL) {
            decoded = g_uri_unescape_string(parts[1], NULL);
            browser_state_set_site_permission(app->browser_state, decoded, parts[2], parts[3]);
            refresh_internal_pages(app);
            g_free(decoded);
        }
        g_strfreev(parts);
    } else if (g_str_has_prefix(message, "remove-site:")) {
        decoded = g_uri_unescape_string(message + 12, NULL);
        if (decoded != NULL) {
            browser_state_remove_site_permissions(app->browser_state, decoded);
            refresh_internal_pages(app);
        }
        g_free(decoded);
    } else if (g_strcmp0(message, "clear-history") == 0) {
        browser_state_clear_history(app->browser_state);
        refresh_internal_pages(app);
    }

    current_title = webkit_web_view_get_title(tab->web_view);
    current_uri = webkit_web_view_get_uri(tab->web_view);
    if (current_uri != NULL && current_title != NULL) {
        update_tab_label(tab);
    }

    g_free(message);
}

static void
on_load_changed(WebKitWebView *web_view,
                WebKitLoadEvent load_event,
                gpointer user_data)
{
    TabState *tab;

    (void)web_view;
    tab = user_data;
    tab->is_loading = load_event == WEBKIT_LOAD_STARTED ||
                      load_event == WEBKIT_LOAD_REDIRECTED ||
                      load_event == WEBKIT_LOAD_COMMITTED;

    if (load_event == WEBKIT_LOAD_FINISHED) {
        const char *title;
        const char *uri;

        title = webkit_web_view_get_title(tab->web_view);
        uri = webkit_web_view_get_uri(tab->web_view);

        if (tab_is_history_page(tab) || tab_is_bookmarks_page(tab) || tab_is_settings_page(tab)) {
            render_internal_page(tab);
        } else if (!tab_is_new_tab(tab) && uri != NULL && *uri != '\0') {
            browser_state_add_history(tab->app->browser_state, title != NULL ? title : uri, uri);
            refresh_internal_pages(tab->app);
        }
    }

    if (tab == get_current_tab(tab->app)) {
        sync_window_to_tab(tab);
    }
}

static gboolean
on_load_failed(WebKitWebView *web_view,
               WebKitLoadEvent load_event,
               const char *failing_uri,
               GError *error,
               gpointer user_data)
{
    TabState *tab;
    GtkAlertDialog *dialog;

    (void)web_view;
    (void)load_event;
    (void)failing_uri;
    tab = user_data;

    if (g_error_matches(error, WEBKIT_NETWORK_ERROR, WEBKIT_NETWORK_ERROR_CANCELLED) ||
        g_error_matches(error,
                        WEBKIT_POLICY_ERROR,
                        WEBKIT_POLICY_ERROR_FRAME_LOAD_INTERRUPTED_BY_POLICY_CHANGE)) {
        return TRUE;
    }

    dialog = gtk_alert_dialog_new("%s", error->message);
    gtk_alert_dialog_set_modal(dialog, TRUE);
    gtk_alert_dialog_show(dialog, GTK_WINDOW(tab->app->window));
    g_object_unref(dialog);

    return FALSE;
}

static const char *
permission_key_for_request(WebKitPermissionRequest *request)
{
    const char *type_name;
    char *lower;
    const char *key;

    type_name = G_OBJECT_TYPE_NAME(request);
    lower = g_ascii_strdown(type_name != NULL ? type_name : "", -1);
    key = "notifications";

    if (strstr(lower, "geolocation") != NULL || strstr(lower, "location") != NULL) {
        key = "location";
    } else if (strstr(lower, "notification") != NULL) {
        key = "notifications";
    } else if (strstr(lower, "camera") != NULL || strstr(lower, "mediastream") != NULL ||
               strstr(lower, "user_media") != NULL || strstr(lower, "usermedia") != NULL) {
        key = "camera";
    } else if (strstr(lower, "microphone") != NULL) {
        key = "microphone";
    }

    g_free(lower);
    return key;
}

static gboolean
on_permission_request(WebKitWebView *web_view,
                      WebKitPermissionRequest *request,
                      gpointer user_data)
{
    TabState *tab;
    char *origin;
    const char *permission;
    const char *decision;

    tab = user_data;
    permission = permission_key_for_request(request);
    origin = origin_from_uri(webkit_web_view_get_uri(web_view));
    decision = browser_state_get_effective_site_permission(tab->app->browser_state, origin, permission);

    if (g_strcmp0(decision, "allow") == 0) {
        webkit_permission_request_allow(request);
    } else {
        webkit_permission_request_deny(request);
    }

    g_free(origin);
    return TRUE;
}

static void
move_toolbar_to_tab(TabState *tab)
{
    GtkWidget *parent;

    if (tab == NULL || tab->app->toolbar == NULL || tab->web_box == NULL) {
        return;
    }

    parent = gtk_widget_get_parent(tab->app->toolbar);
    if (parent == tab->web_box) {
        return;
    }

    if (parent != NULL) {
        g_object_ref(tab->app->toolbar);
        gtk_box_remove(GTK_BOX(parent), tab->app->toolbar);
        gtk_box_prepend(GTK_BOX(tab->web_box), tab->app->toolbar);
        g_object_unref(tab->app->toolbar);
        return;
    }

    gtk_box_prepend(GTK_BOX(tab->web_box), tab->app->toolbar);
}

static void
on_switch_page(GtkNotebook *notebook,
               GtkWidget *page,
               guint page_num,
               gpointer user_data)
{
    TabState *tab;

    (void)notebook;
    (void)page_num;
    (void)user_data;
    tab = get_tab_from_page(page);
    move_toolbar_to_tab(tab);
    sync_window_to_tab(tab);
}

static void
on_bookmark_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    TabState *tab;
    const char *title;
    const char *uri;

    (void)button;
    app = user_data;
    tab = get_current_tab(app);
    if (!tab_can_bookmark(tab)) {
        return;
    }

    title = webkit_web_view_get_title(tab->web_view);
    uri = webkit_web_view_get_uri(tab->web_view);
    browser_state_toggle_bookmark(app->browser_state, title != NULL ? title : uri, uri);
    update_bookmark_button(app);
    refresh_internal_pages(app);
}

static void
on_open_history_clicked(GtkButton *button, gpointer user_data)
{
    (void)button;
    load_uri_in_current_tab(user_data, ((AppState *)user_data)->history_uri);
}

static void
on_open_home_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    const char *homepage_uri;

    (void)button;
    app = user_data;
    homepage_uri = browser_state_get_homepage_uri(app->browser_state);
    if (homepage_uri != NULL && *homepage_uri != '\0') {
        load_uri_in_current_tab(app, homepage_uri);
        return;
    }

    load_uri_in_current_tab(app, app->new_tab_uri);
}

static void
on_open_bookmarks_clicked(GtkButton *button, gpointer user_data)
{
    (void)button;
    load_uri_in_current_tab(user_data, ((AppState *)user_data)->bookmarks_uri);
}

static void
on_open_settings_clicked(GtkButton *button, gpointer user_data)
{
    (void)button;
    load_uri_in_current_tab(user_data, ((AppState *)user_data)->settings_uri);
}

static void
on_close_tab_clicked(GtkButton *button, gpointer user_data)
{
    TabState *tab;

    (void)button;
    tab = user_data;
    close_tab(tab);
}

static void
on_tab_middle_click_pressed(GtkGestureClick *gesture,
                            int n_press,
                            double x,
                            double y,
                            gpointer user_data)
{
    (void)gesture;
    (void)n_press;
    (void)x;
    (void)y;
    on_close_tab_clicked(NULL, user_data);
}

static gboolean
on_window_key_pressed(GtkEventControllerKey *controller,
                      guint keyval,
                      guint keycode,
                      GdkModifierType state,
                      gpointer user_data)
{
    AppState *app;

    (void)controller;
    (void)keycode;
    app = user_data;

    if ((state & GDK_CONTROL_MASK) == 0) {
        if (keyval == GDK_KEY_Escape) {
            TabState *tab;

            tab = get_current_tab(app);
            if (tab != NULL && tab->is_loading) {
                webkit_web_view_stop_loading(tab->web_view);
                return TRUE;
            }
        }

        return FALSE;
    }

    if (keyval == GDK_KEY_t || keyval == GDK_KEY_T) {
        on_new_tab_clicked(NULL, app);
        return TRUE;
    }

    if (keyval == GDK_KEY_l || keyval == GDK_KEY_L) {
        gtk_widget_grab_focus(app->address_entry);
        gtk_editable_select_region(GTK_EDITABLE(app->address_entry), 0, -1);
        return TRUE;
    }

    if (keyval == GDK_KEY_r || keyval == GDK_KEY_R) {
        on_reload_clicked(NULL, app);
        return TRUE;
    }

    if (keyval == GDK_KEY_d || keyval == GDK_KEY_D) {
        on_bookmark_clicked(NULL, app);
        return TRUE;
    }

    if (keyval == GDK_KEY_h || keyval == GDK_KEY_H) {
        on_open_history_clicked(NULL, app);
        return TRUE;
    }

    if (keyval == GDK_KEY_w || keyval == GDK_KEY_W) {
        close_tab(get_current_tab(app));
        return TRUE;
    }

    if (keyval == GDK_KEY_Tab || keyval == GDK_KEY_Page_Down) {
        switch_tab(app, (state & GDK_SHIFT_MASK) != 0 ? -1 : 1);
        return TRUE;
    }

    if (keyval == GDK_KEY_ISO_Left_Tab || keyval == GDK_KEY_Page_Up) {
        switch_tab(app, -1);
        return TRUE;
    }

    return FALSE;
}

static TabState *
create_tab(AppState *app, const char *uri)
{
    GtkGesture *middle_click;
    GtkWidget *close_button;
    GtkWidget *favicon_image;
    GtkWidget *tab_box;
    GtkWidget *tab_revealer;
    GtkWidget *title_label;
    GtkWidget *web_box;
    GtkWidget *web_view;
    TabState *tab;
    WebKitUserContentManager *content_manager;

    tab = g_new0(TabState, 1);
    tab->app = app;

    web_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    web_view = g_object_new(WEBKIT_TYPE_WEB_VIEW, NULL);
    tab->page = web_box;
    tab->web_box = web_box;
    tab->web_view = WEBKIT_WEB_VIEW(web_view);
    configure_web_view(tab->web_view);
    content_manager = webkit_web_view_get_user_content_manager(tab->web_view);
    g_signal_connect(content_manager,
                     "script-message-received::ubar",
                     G_CALLBACK(on_script_message_received),
                     tab);
    webkit_user_content_manager_register_script_message_handler(content_manager, "ubar", NULL);

    favicon_image = gtk_image_new_from_icon_name("globe-symbolic");
    title_label = gtk_label_new("New Tab");
    gtk_label_set_ellipsize(GTK_LABEL(title_label), PANGO_ELLIPSIZE_END);
    gtk_label_set_max_width_chars(GTK_LABEL(title_label), 32);
    gtk_label_set_xalign(GTK_LABEL(title_label), 0.0f);
    gtk_widget_set_hexpand(title_label, TRUE);
    tab->favicon_image = favicon_image;
    tab->title_label = title_label;

    tab_box = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 8);
    gtk_widget_set_margin_top(tab_box, 6);
    gtk_widget_set_margin_bottom(tab_box, 6);
    gtk_widget_set_margin_start(tab_box, 10);
    gtk_widget_set_margin_end(tab_box, 10);
    gtk_widget_set_size_request(tab_box, 220, -1);

    close_button = gtk_button_new_from_icon_name("window-close-symbolic");
    gtk_button_set_has_frame(GTK_BUTTON(close_button), FALSE);
    gtk_widget_set_focusable(close_button, FALSE);
    gtk_widget_set_halign(close_button, GTK_ALIGN_END);

    gtk_box_append(GTK_BOX(tab_box), favicon_image);
    gtk_box_append(GTK_BOX(tab_box), title_label);
    gtk_box_append(GTK_BOX(tab_box), close_button);

    tab_revealer = gtk_revealer_new();
    gtk_revealer_set_transition_type(GTK_REVEALER(tab_revealer),
                                     GTK_REVEALER_TRANSITION_TYPE_SLIDE_RIGHT);
    gtk_revealer_set_transition_duration(GTK_REVEALER(tab_revealer), 140);
    gtk_revealer_set_child(GTK_REVEALER(tab_revealer), tab_box);
    gtk_revealer_set_reveal_child(GTK_REVEALER(tab_revealer), FALSE);
    tab->tab_revealer = tab_revealer;
    tab->tab_box = tab_box;

    gtk_widget_set_hexpand(web_view, TRUE);
    gtk_widget_set_vexpand(web_view, TRUE);
    gtk_box_append(GTK_BOX(web_box), web_view);

    g_object_set_data_full(G_OBJECT(web_box), "tab-state", tab, g_free);

    gtk_notebook_append_page(GTK_NOTEBOOK(app->notebook), web_box, tab_revealer);
    gtk_notebook_set_tab_reorderable(GTK_NOTEBOOK(app->notebook), web_box, TRUE);

    middle_click = gtk_gesture_click_new();
    gtk_gesture_single_set_button(GTK_GESTURE_SINGLE(middle_click), GDK_BUTTON_MIDDLE);
    gtk_widget_add_controller(tab_revealer, GTK_EVENT_CONTROLLER(middle_click));

    g_signal_connect(close_button, "clicked", G_CALLBACK(on_close_tab_clicked), tab);
    g_signal_connect(middle_click, "pressed", G_CALLBACK(on_tab_middle_click_pressed), tab);
    g_signal_connect(tab->web_view, "notify::uri", G_CALLBACK(on_uri_changed), tab);
    g_signal_connect(tab->web_view, "notify::title", G_CALLBACK(on_title_changed), tab);
    g_signal_connect(tab->web_view, "notify::favicon", G_CALLBACK(on_favicon_changed), tab);
    g_signal_connect(tab->web_view, "load-changed", G_CALLBACK(on_load_changed), tab);
    g_signal_connect(tab->web_view, "load-failed", G_CALLBACK(on_load_failed), tab);
    g_signal_connect(tab->web_view, "permission-request", G_CALLBACK(on_permission_request), tab);

    update_tab_favicon(tab);
    update_tab_label(tab);
    if (g_strcmp0(uri, app->new_tab_uri) == 0) {
        load_new_tab_page(tab);
    } else {
        webkit_web_view_load_uri(tab->web_view, uri);
    }
    g_idle_add(reveal_tab_after_create, tab_revealer);

    return tab;
}

static void
on_new_tab_clicked(GtkButton *button, gpointer user_data)
{
    AppState *app;
    TabState *tab;
    int page_num;

    (void)button;
    app = user_data;
    tab = create_tab(app, app->new_tab_uri);
    move_toolbar_to_tab(tab);
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), tab->page);
    gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), page_num);
}

static void
on_activate(GtkApplication *gtk_app, gpointer user_data)
{
    GtkEventController *key_controller;
    GtkWidget *menu_box;
    GtkWidget *menu_popover;
    GtkWidget *toolbar;
    GtkWidget *top_controls;
    GtkWidget *vbox;
    AppState *app;
    TabState *tab;
    int page_num;

    (void)user_data;

    app = g_new0(AppState, 1);
    app->browser_state = browser_state_new();
    app->new_tab_uri = build_asset_uri("assets/newtab/index.html");
    app->history_uri = build_asset_uri("assets/pages/history/index.html");
    app->bookmarks_uri = build_asset_uri("assets/pages/bookmarks/index.html");
    app->settings_uri = build_asset_uri("assets/pages/settings/index.html");

    app->window = gtk_application_window_new(gtk_app);
    gtk_window_set_default_size(GTK_WINDOW(app->window), 1200, 800);
    gtk_window_set_title(GTK_WINDOW(app->window), "ubar");
    gtk_window_set_decorated(GTK_WINDOW(app->window), FALSE);

    vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    toolbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    app->toolbar = toolbar;
    gtk_widget_set_margin_top(toolbar, 6);
    gtk_widget_set_margin_bottom(toolbar, 6);
    gtk_widget_set_margin_start(toolbar, 6);
    gtk_widget_set_margin_end(toolbar, 6);

    app->back_button = gtk_button_new_from_icon_name("go-previous-symbolic");
    app->forward_button = gtk_button_new_from_icon_name("go-next-symbolic");
    app->reload_button = gtk_button_new_from_icon_name("view-refresh-symbolic");
    app->address_entry = gtk_entry_new();
    app->notebook = gtk_notebook_new();
    app->bookmark_button = gtk_button_new_from_icon_name("bookmark-new-symbolic");
    app->new_tab_button = gtk_button_new_from_icon_name("list-add-symbolic");
    app->menu_button = gtk_menu_button_new();
    app->window_controls = gtk_window_controls_new(GTK_PACK_END);
    top_controls = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
    app->tab_action_box = top_controls;

    gtk_button_set_has_frame(GTK_BUTTON(app->new_tab_button), FALSE);
    gtk_button_set_has_frame(GTK_BUTTON(app->bookmark_button), FALSE);
    gtk_menu_button_set_icon_name(GTK_MENU_BUTTON(app->menu_button), "open-menu-symbolic");
    gtk_widget_set_hexpand(app->address_entry, TRUE);
    gtk_notebook_set_scrollable(GTK_NOTEBOOK(app->notebook), TRUE);
    gtk_box_append(GTK_BOX(top_controls), app->new_tab_button);
    gtk_box_append(GTK_BOX(top_controls), app->window_controls);
    gtk_notebook_set_action_widget(GTK_NOTEBOOK(app->notebook),
                                   top_controls,
                                   GTK_PACK_END);

    gtk_box_append(GTK_BOX(toolbar), app->back_button);
    gtk_box_append(GTK_BOX(toolbar), app->forward_button);
    gtk_box_append(GTK_BOX(toolbar), app->reload_button);
    gtk_box_append(GTK_BOX(toolbar), app->address_entry);
    gtk_box_append(GTK_BOX(toolbar), app->bookmark_button);
    gtk_box_append(GTK_BOX(toolbar), app->menu_button);

    gtk_box_append(GTK_BOX(vbox), app->notebook);
    gtk_window_set_child(GTK_WINDOW(app->window), vbox);

    key_controller = gtk_event_controller_key_new();
    gtk_widget_add_controller(app->window, key_controller);

    menu_box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 4);
    gtk_widget_set_margin_top(menu_box, 8);
    gtk_widget_set_margin_bottom(menu_box, 8);
    gtk_widget_set_margin_start(menu_box, 8);
    gtk_widget_set_margin_end(menu_box, 8);
    app->home_menu_item = gtk_button_new_with_label("Home");
    app->history_menu_item = gtk_button_new_with_label("History");
    app->bookmarks_menu_item = gtk_button_new_with_label("Bookmarks");
    app->settings_menu_item = gtk_button_new_with_label("Settings");
    gtk_box_append(GTK_BOX(menu_box), app->home_menu_item);
    gtk_box_append(GTK_BOX(menu_box), app->history_menu_item);
    gtk_box_append(GTK_BOX(menu_box), app->bookmarks_menu_item);
    gtk_box_append(GTK_BOX(menu_box), app->settings_menu_item);
    menu_popover = gtk_popover_new();
    gtk_popover_set_child(GTK_POPOVER(menu_popover), menu_box);
    gtk_menu_button_set_popover(GTK_MENU_BUTTON(app->menu_button), menu_popover);

    g_signal_connect(app->back_button, "clicked", G_CALLBACK(on_back_clicked), app);
    g_signal_connect(app->forward_button, "clicked", G_CALLBACK(on_forward_clicked), app);
    g_signal_connect(app->reload_button, "clicked", G_CALLBACK(on_reload_clicked), app);
    g_signal_connect(app->address_entry, "activate", G_CALLBACK(on_address_activate), app);
    g_signal_connect(app->bookmark_button, "clicked", G_CALLBACK(on_bookmark_clicked), app);
    g_signal_connect(app->new_tab_button, "clicked", G_CALLBACK(on_new_tab_clicked), app);
    g_signal_connect(app->home_menu_item, "clicked", G_CALLBACK(on_open_home_clicked), app);
    g_signal_connect(app->history_menu_item, "clicked", G_CALLBACK(on_open_history_clicked), app);
    g_signal_connect(app->bookmarks_menu_item, "clicked", G_CALLBACK(on_open_bookmarks_clicked), app);
    g_signal_connect(app->settings_menu_item, "clicked", G_CALLBACK(on_open_settings_clicked), app);
    g_signal_connect(app->notebook, "switch-page", G_CALLBACK(on_switch_page), app);
    g_signal_connect(key_controller, "key-pressed", G_CALLBACK(on_window_key_pressed), app);
    g_signal_connect(app->window, "destroy", G_CALLBACK(free_app_state), app);

    tab = create_tab(app, app->new_tab_uri);
    move_toolbar_to_tab(tab);
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), tab->page);
    gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), page_num);
    sync_window_to_tab(tab);
    update_bookmark_button(app);

    gtk_window_present(GTK_WINDOW(app->window));
}

int
browser_app_run(int argc, char *argv[])
{
    GtkApplication *app;
    int status;

    app = gtk_application_new("dev.ghanti.ubar", G_APPLICATION_DEFAULT_FLAGS);
    g_signal_connect(app, "activate", G_CALLBACK(on_activate), NULL);

    status = g_application_run(G_APPLICATION(app), argc, argv);
    g_object_unref(app);

    return status;
}
