#include <gtk/gtk.h>
#include <string.h>
#include <webkit/webkit.h>

typedef struct AppState AppState;
typedef struct TabState TabState;

static gboolean remove_tab_after_animation(gpointer user_data);
static void on_new_tab_clicked(GtkButton *button, gpointer user_data);

struct AppState {
    GtkWidget *window;
    GtkWidget *notebook;
    GtkWidget *back_button;
    GtkWidget *forward_button;
    GtkWidget *reload_button;
    GtkWidget *address_entry;
    GtkWidget *new_tab_button;
};

struct TabState {
    AppState *app;
    GtkWidget *page;
    GtkWidget *tab_revealer;
    GtkWidget *tab_box;
    GtkWidget *favicon_image;
    GtkWidget *title_label;
    WebKitWebView *web_view;
    gboolean is_loading;
};

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

    gtk_editable_set_text(GTK_EDITABLE(app->address_entry), uri != NULL ? uri : "about:blank");
    gtk_widget_set_sensitive(app->back_button, webkit_web_view_can_go_back(tab->web_view));
    gtk_widget_set_sensitive(app->forward_button, webkit_web_view_can_go_forward(tab->web_view));
    gtk_button_set_icon_name(GTK_BUTTON(app->reload_button),
                             tab->is_loading
                                 ? "process-stop-symbolic"
                                 : "view-refresh-symbolic");
    gtk_window_set_title(GTK_WINDOW(app->window), title != NULL ? title : "ubar");
}

static void
configure_web_view(WebKitWebView *web_view)
{
    WebKitSettings *settings;

    settings = webkit_web_view_get_settings(web_view);
    webkit_settings_set_hardware_acceleration_policy(
        settings,
        WEBKIT_HARDWARE_ACCELERATION_POLICY_NEVER);
    webkit_settings_set_enable_webgl(settings, FALSE);
    webkit_settings_set_enable_2d_canvas_acceleration(settings, FALSE);
    webkit_settings_set_enable_write_console_messages_to_stdout(settings, TRUE);
    webkit_settings_set_enable_developer_extras(settings, TRUE);
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

static gboolean
on_permission_request(WebKitWebView *web_view,
                      WebKitPermissionRequest *request,
                      gpointer user_data)
{
    (void)web_view;
    (void)user_data;

    webkit_permission_request_deny(request);
    return TRUE;
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
    sync_window_to_tab(tab);
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
    GtkWidget *web_view;
    GtkWidget *favicon_image;
    GtkWidget *tab_box;
    GtkWidget *close_button;
    GtkWidget *tab_revealer;
    GtkWidget *title_label;
    TabState *tab;

    tab = g_new0(TabState, 1);
    tab->app = app;

    web_view = webkit_web_view_new();
    tab->page = web_view;
    tab->web_view = WEBKIT_WEB_VIEW(web_view);
    configure_web_view(tab->web_view);

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

    g_object_set_data_full(G_OBJECT(web_view), "tab-state", tab, g_free);

    gtk_notebook_append_page(GTK_NOTEBOOK(app->notebook), web_view, tab_revealer);
    gtk_notebook_set_tab_reorderable(GTK_NOTEBOOK(app->notebook), web_view, TRUE);

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
    webkit_web_view_load_uri(tab->web_view, uri);
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
    tab = create_tab(app, "about:blank");
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(app->notebook), tab->page);
    gtk_notebook_set_current_page(GTK_NOTEBOOK(app->notebook), page_num);
}

static void
on_activate(GtkApplication *app, gpointer user_data)
{
    GtkEventController *key_controller;
    GtkWidget *window;
    GtkWidget *vbox;
    GtkWidget *toolbar;
    AppState *state;
    TabState *tab;
    int page_num;

    (void)user_data;

    state = g_new0(AppState, 1);

    window = gtk_application_window_new(app);
    gtk_window_set_default_size(GTK_WINDOW(window), 1200, 800);
    gtk_window_set_title(GTK_WINDOW(window), "ubar");

    vbox = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    toolbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_widget_set_margin_top(toolbar, 6);
    gtk_widget_set_margin_bottom(toolbar, 6);
    gtk_widget_set_margin_start(toolbar, 6);
    gtk_widget_set_margin_end(toolbar, 6);

    state->window = window;
    state->back_button = gtk_button_new_from_icon_name("go-previous-symbolic");
    state->forward_button = gtk_button_new_from_icon_name("go-next-symbolic");
    state->reload_button = gtk_button_new_from_icon_name("view-refresh-symbolic");
    state->address_entry = gtk_entry_new();
    state->notebook = gtk_notebook_new();
    state->new_tab_button = gtk_button_new_from_icon_name("list-add-symbolic");

    gtk_button_set_has_frame(GTK_BUTTON(state->new_tab_button), FALSE);
    gtk_widget_set_hexpand(state->address_entry, TRUE);
    gtk_notebook_set_scrollable(GTK_NOTEBOOK(state->notebook), TRUE);
    gtk_notebook_set_action_widget(GTK_NOTEBOOK(state->notebook),
                                   state->new_tab_button,
                                   GTK_PACK_END);

    gtk_box_append(GTK_BOX(toolbar), state->back_button);
    gtk_box_append(GTK_BOX(toolbar), state->forward_button);
    gtk_box_append(GTK_BOX(toolbar), state->reload_button);
    gtk_box_append(GTK_BOX(toolbar), state->address_entry);

    gtk_box_append(GTK_BOX(vbox), toolbar);
    gtk_box_append(GTK_BOX(vbox), state->notebook);
    gtk_window_set_child(GTK_WINDOW(window), vbox);
    key_controller = gtk_event_controller_key_new();
    gtk_widget_add_controller(window, key_controller);

    g_signal_connect(state->back_button, "clicked", G_CALLBACK(on_back_clicked), state);
    g_signal_connect(state->forward_button, "clicked", G_CALLBACK(on_forward_clicked), state);
    g_signal_connect(state->reload_button, "clicked", G_CALLBACK(on_reload_clicked), state);
    g_signal_connect(state->address_entry, "activate", G_CALLBACK(on_address_activate), state);
    g_signal_connect(state->new_tab_button, "clicked", G_CALLBACK(on_new_tab_clicked), state);
    g_signal_connect(state->notebook, "switch-page", G_CALLBACK(on_switch_page), state);
    g_signal_connect(key_controller, "key-pressed", G_CALLBACK(on_window_key_pressed), state);
    g_signal_connect_swapped(window, "destroy", G_CALLBACK(g_free), state);

    tab = create_tab(state, "about:blank");
    page_num = gtk_notebook_page_num(GTK_NOTEBOOK(state->notebook), tab->page);
    gtk_notebook_set_current_page(GTK_NOTEBOOK(state->notebook), page_num);
    sync_window_to_tab(tab);

    gtk_window_present(GTK_WINDOW(window));
}

int
main(int argc, char *argv[])
{
    GtkApplication *app;
    int status;

    app = gtk_application_new("dev.ghanti.ubar", G_APPLICATION_DEFAULT_FLAGS);
    g_signal_connect(app, "activate", G_CALLBACK(on_activate), NULL);

    status = g_application_run(G_APPLICATION(app), argc, argv);
    g_object_unref(app);

    return status;
}
