#include <gtk/gtk.h>
#include <string.h>
#include <webkit/webkit.h>

typedef struct {
    GtkWidget *window;
    GtkWidget *back_button;
    GtkWidget *forward_button;
    GtkWidget *reload_button;
    GtkWidget *address_entry;
    WebKitWebView *web_view;
    gboolean is_loading;
} AppState;

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

static void
update_navigation(AppState *state)
{
    gtk_widget_set_sensitive(state->back_button,
                             webkit_web_view_can_go_back(state->web_view));
    gtk_widget_set_sensitive(state->forward_button,
                             webkit_web_view_can_go_forward(state->web_view));
}

static void
load_address(AppState *state)
{
    const char *input;
    char *uri;

    input = gtk_editable_get_text(GTK_EDITABLE(state->address_entry));
    uri = normalize_uri(input);
    if (uri == NULL) {
        return;
    }

    webkit_web_view_load_uri(state->web_view, uri);
    g_free(uri);
}

static void
on_back_clicked(GtkButton *button, gpointer user_data)
{
    AppState *state;

    (void)button;
    state = user_data;
    webkit_web_view_go_back(state->web_view);
}

static void
on_forward_clicked(GtkButton *button, gpointer user_data)
{
    AppState *state;

    (void)button;
    state = user_data;
    webkit_web_view_go_forward(state->web_view);
}

static void
on_reload_clicked(GtkButton *button, gpointer user_data)
{
    AppState *state;

    (void)button;
    state = user_data;
    if (state->is_loading) {
        webkit_web_view_stop_loading(state->web_view);
        return;
    }

    webkit_web_view_reload(state->web_view);
}

static void
on_address_activate(GtkEntry *entry, gpointer user_data)
{
    AppState *state;

    (void)entry;
    state = user_data;
    load_address(state);
}

static void
on_uri_changed(WebKitWebView *web_view, GParamSpec *pspec, gpointer user_data)
{
    AppState *state;
    const char *uri;

    (void)pspec;
    state = user_data;
    uri = webkit_web_view_get_uri(web_view);
    if (uri != NULL) {
        gtk_editable_set_text(GTK_EDITABLE(state->address_entry), uri);
    }
}

static void
on_title_changed(WebKitWebView *web_view, GParamSpec *pspec, gpointer user_data)
{
    AppState *state;
    const char *title;

    (void)pspec;
    state = user_data;
    title = webkit_web_view_get_title(web_view);
    gtk_window_set_title(GTK_WINDOW(state->window), title != NULL ? title : "ubar");
}

static void
on_load_changed(WebKitWebView *web_view,
                WebKitLoadEvent load_event,
                gpointer user_data)
{
    AppState *state;

    (void)web_view;
    state = user_data;
    state->is_loading = load_event == WEBKIT_LOAD_STARTED ||
                        load_event == WEBKIT_LOAD_REDIRECTED ||
                        load_event == WEBKIT_LOAD_COMMITTED;

    gtk_button_set_icon_name(GTK_BUTTON(state->reload_button),
                             state->is_loading
                                 ? "process-stop-symbolic"
                                 : "view-refresh-symbolic");
    update_navigation(state);
}

static gboolean
on_load_failed(WebKitWebView *web_view,
               WebKitLoadEvent load_event,
               const char *failing_uri,
               GError *error,
               gpointer user_data)
{
    AppState *state;
    GtkAlertDialog *dialog;

    (void)web_view;
    (void)load_event;
    (void)failing_uri;
    state = user_data;

    dialog = gtk_alert_dialog_new("%s", error->message);
    gtk_alert_dialog_set_modal(dialog, TRUE);
    gtk_alert_dialog_show(dialog, GTK_WINDOW(state->window));
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
on_activate(GtkApplication *app, gpointer user_data)
{
    GtkWidget *window;
    GtkWidget *vbox;
    GtkWidget *toolbar;
    GtkWidget *web_view;
    AppState *state;

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
    web_view = webkit_web_view_new();
    state->web_view = WEBKIT_WEB_VIEW(web_view);

    gtk_editable_set_text(GTK_EDITABLE(state->address_entry), "https://example.com");
    gtk_widget_set_hexpand(state->address_entry, TRUE);
    gtk_widget_set_hexpand(web_view, TRUE);
    gtk_widget_set_vexpand(web_view, TRUE);

    gtk_box_append(GTK_BOX(toolbar), state->back_button);
    gtk_box_append(GTK_BOX(toolbar), state->forward_button);
    gtk_box_append(GTK_BOX(toolbar), state->reload_button);
    gtk_box_append(GTK_BOX(toolbar), state->address_entry);

    gtk_box_append(GTK_BOX(vbox), toolbar);
    gtk_box_append(GTK_BOX(vbox), web_view);
    gtk_window_set_child(GTK_WINDOW(window), vbox);

    g_signal_connect(state->back_button, "clicked", G_CALLBACK(on_back_clicked), state);
    g_signal_connect(state->forward_button, "clicked", G_CALLBACK(on_forward_clicked), state);
    g_signal_connect(state->reload_button, "clicked", G_CALLBACK(on_reload_clicked), state);
    g_signal_connect(state->address_entry, "activate", G_CALLBACK(on_address_activate), state);
    g_signal_connect(state->web_view, "notify::uri", G_CALLBACK(on_uri_changed), state);
    g_signal_connect(state->web_view, "notify::title", G_CALLBACK(on_title_changed), state);
    g_signal_connect(state->web_view, "load-changed", G_CALLBACK(on_load_changed), state);
    g_signal_connect(state->web_view, "load-failed", G_CALLBACK(on_load_failed), state);
    g_signal_connect(state->web_view, "permission-request", G_CALLBACK(on_permission_request), state);
    g_signal_connect_swapped(window, "destroy", G_CALLBACK(g_free), state);

    update_navigation(state);
    webkit_web_view_load_uri(state->web_view, "https://example.com");

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
