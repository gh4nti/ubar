#include <dlfcn.h>
#include <gtk/gtk.h>
#include <json-glib/json-glib.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include "ubar_engine.h"

typedef struct Shell Shell;

typedef struct Tab {
    Shell *shell;
    GtkWidget *page;
    GtkWidget *label;
    UbarView view;
    char *uri;
    double zoom;
} Tab;

struct Shell {
    GtkWidget *window;
    GtkWidget *notebook;
    GtkWidget *address;
    GtkWidget *extension_actions;
    GPtrArray *tabs;
    void *engine_library;
    const UbarEngineApiV1 *engine;
    UbarProfile profile;
    guint memory_timer;
    bool private_mode;
};

static char *extension_control(Shell *shell, const char *request_json);
static char *browser_control(Shell *shell, const char *request_json);
static void populate_extension_actions(Shell *shell);
static void add_tab(Shell *shell);

static uint64_t physical_memory(void) {
    long pages = sysconf(_SC_PHYS_PAGES);
    long page_size = sysconf(_SC_PAGESIZE);
    if (pages <= 0 || page_size <= 0) return 1024ull * 1024 * 1024;
    return (uint64_t)pages * (uint64_t)page_size;
}

static Tab *current_tab(Shell *shell) {
    int page = gtk_notebook_get_current_page(GTK_NOTEBOOK(shell->notebook));
    if (page < 0 || (guint)page >= shell->tabs->len) return NULL;
    return g_ptr_array_index(shell->tabs, page);
}

static char *event_text(const UbarEventV1 *event) {
    if (!event->text_utf8.data || !event->text_utf8.len) return g_strdup("");
    return g_strndup((const char *)event->text_utf8.data, event->text_utf8.len);
}

static void on_engine_event(void *user_data, const UbarEventV1 *event) {
    Tab *tab = user_data;
    if (!tab || !event) return;
    char *text = event_text(event);
    if (event->kind == UBAR_TITLE_CHANGED && *text) {
        char *short_title = g_utf8_substring(text, 0, MIN(g_utf8_strlen(text, -1), 32));
        gtk_label_set_text(GTK_LABEL(tab->label), short_title);
        g_free(short_title);
    } else if (event->kind == UBAR_URI_CHANGED && current_tab(tab->shell) == tab) {
        g_free(tab->uri);
        tab->uri = g_strdup(text);
        gtk_entry_set_text(GTK_ENTRY(tab->shell->address), text);
    } else if (event->kind == UBAR_URI_CHANGED) {
        g_free(tab->uri);
        tab->uri = g_strdup(text);
    } else if (event->kind == UBAR_RENDERER_CRASHED) {
        gtk_label_set_text(GTK_LABEL(tab->label), "Crashed");
    } else if (event->kind == UBAR_DOWNLOAD_REQUESTED && *text && !tab->shell->private_mode) {
        GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(tab->shell->window), GTK_DIALOG_MODAL,
            GTK_MESSAGE_QUESTION, GTK_BUTTONS_YES_NO,
            "%s", "Install this signed browser extension?");
        if (gtk_dialog_run(GTK_DIALOG(dialog)) == GTK_RESPONSE_YES) {
            JsonBuilder *builder = json_builder_new();
            json_builder_begin_object(builder);
            json_builder_set_member_name(builder, "operation");
            json_builder_add_string_value(builder, "installPackage");
            json_builder_set_member_name(builder, "path");
            json_builder_add_string_value(builder, text);
            json_builder_end_object(builder);
            JsonGenerator *generator = json_generator_new();
            JsonNode *root = json_builder_get_root(builder);
            json_generator_set_root(generator, root);
            char *request = json_generator_to_data(generator, NULL);
            char *response = extension_control(tab->shell, request);
            if (response) populate_extension_actions(tab->shell);
            g_free(response);
            g_free(request);
            json_node_free(root);
            g_object_unref(generator);
            g_object_unref(builder);
        }
        gtk_widget_destroy(dialog);
    }
    g_free(text);
}

static void add_tab(Shell *shell) {
    Tab *tab = g_new0(Tab, 1);
    tab->shell = shell;
    tab->zoom = 1.0;
    tab->page = gtk_box_new(GTK_ORIENTATION_VERTICAL, 0);
    tab->label = gtk_label_new("New Tab");
    gtk_notebook_append_page(GTK_NOTEBOOK(shell->notebook), tab->page, tab->label);
    g_ptr_array_add(shell->tabs, tab);
    gtk_widget_show(tab->page);

    if (shell->engine && shell->profile) {
        GtkAllocation size;
        gtk_widget_get_allocation(shell->notebook, &size);
        UbarViewConfigV1 view = {
            .struct_size = sizeof(view),
            .native_parent = tab->page,
            .width = MAX(size.width, 1),
            .height = MAX(size.height, 1),
            .device_scale = gtk_widget_get_scale_factor(shell->window),
            .initially_visible = true,
        };
        UbarCallbacksV1 callbacks = {
            .struct_size = sizeof(callbacks),
            .user_data = tab,
            .event = on_engine_event,
        };
        if (shell->engine->create_view(shell->profile, &view, &callbacks, &tab->view) != UBAR_OK)
            gtk_box_pack_start(GTK_BOX(tab->page), gtk_label_new("WebKit engine unavailable"), TRUE, TRUE, 0);
    }
    gtk_widget_show_all(tab->page);
    gtk_notebook_set_current_page(GTK_NOTEBOOK(shell->notebook), shell->tabs->len - 1);
}

static void on_add_tab(GtkButton *button, gpointer data) {
    (void)button;
    add_tab(data);
}

static void on_address(GtkEntry *entry, gpointer data) {
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    const char *text = gtk_entry_get_text(entry);
    if (!shell->engine || !tab || !tab->view || !text || !*text) return;
    char *uri = NULL;
    if (strstr(text, "://")) uri = g_strdup(text);
    else if (strchr(text, ' ') || !strchr(text, '.')) {
        char *escaped = g_uri_escape_string(text, NULL, TRUE);
        uri = g_strdup_printf("https://www.google.com/search?q=%s", escaped);
        g_free(escaped);
    } else uri = g_strdup_printf("https://%s", text);
    UbarBytes bytes = {(const uint8_t *)uri, strlen(uri)};
    shell->engine->navigate(tab->view, bytes);
    g_free(uri);
}

static void on_reload(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    if (shell->engine && tab && tab->view) shell->engine->reload(tab->view);
}

static void on_stop(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    if (shell->engine && tab && tab->view) shell->engine->stop(tab->view);
}

static void change_zoom(Shell *shell, double delta) {
    Tab *tab = current_tab(shell);
    if (!shell->engine || !tab || !tab->view) return;
    tab->zoom = CLAMP(((int)((tab->zoom + delta) * 10 + .5)) / 10.0, .5, 5.0);
    shell->engine->set_zoom(tab->view, tab->zoom);
}

static void on_zoom_out(GtkButton *button, gpointer data) { (void)button; change_zoom(data, -.1); }
static void on_zoom_in(GtkButton *button, gpointer data) { (void)button; change_zoom(data, .1); }

static void on_close_tab(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    int index = gtk_notebook_get_current_page(GTK_NOTEBOOK(shell->notebook));
    if (index < 0 || (guint)index >= shell->tabs->len) return;
    Tab *tab = g_ptr_array_index(shell->tabs, index);
    if (tab->view) shell->engine->destroy_view(tab->view);
    g_ptr_array_remove_index(shell->tabs, index);
    gtk_notebook_remove_page(GTK_NOTEBOOK(shell->notebook), index);
    g_free(tab->uri);
    g_free(tab);
    if (!shell->tabs->len) add_tab(shell);
}

static void on_back(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    if (shell->engine && tab && tab->view) shell->engine->go_back(tab->view);
}

static void on_forward(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    if (shell->engine && tab && tab->view) shell->engine->go_forward(tab->view);
}

static void on_switch_page(GtkNotebook *notebook, GtkWidget *page, guint index, gpointer data) {
    (void)notebook;
    (void)page;
    Shell *shell = data;
    for (guint i = 0; i < shell->tabs->len; ++i) {
        Tab *tab = g_ptr_array_index(shell->tabs, i);
        if (tab->view) shell->engine->set_visible(tab->view, i == index);
    }
    Tab *tab = current_tab(shell);
    gtk_entry_set_text(GTK_ENTRY(shell->address), tab && tab->uri ? tab->uri : "");
}

static void open_private(GtkButton *button, gpointer data) {
    (void)button;
    (void)data;
    char *executable = g_file_read_link("/proc/self/exe", NULL);
    if (!executable) return;
    char *argv[] = {executable, "--incognito", NULL};
    g_spawn_async(NULL, argv, NULL, G_SPAWN_DO_NOT_REAP_CHILD, NULL, NULL, NULL, NULL);
    g_free(executable);
}

static void load_engine(Shell *shell) {
    shell->engine_library = dlopen("libubar_engine.so", RTLD_NOW | RTLD_LOCAL);
    if (!shell->engine_library) return;
    UbarGetEngineApi get_api = (UbarGetEngineApi)dlsym(shell->engine_library, "ubar_get_engine_api");
    if (!get_api || get_api(UBAR_ENGINE_ABI_V1, &shell->engine) != UBAR_OK) shell->engine = NULL;
}

static char *extension_control(Shell *shell, const char *request_json) {
    if (!shell->engine || !shell->profile || !shell->engine->extension_control_json) return NULL;
    UbarBytes request = {(const uint8_t *)request_json, strlen(request_json)};
    UbarOwnedBytes response = {0};
    if (shell->engine->extension_control_json(shell->profile, request, &response) != UBAR_OK ||
        !response.data) return NULL;
    char *copy = g_strndup((const char *)response.data, response.len);
    shell->engine->free_bytes(response);
    return copy;
}

static char *browser_control(Shell *shell, const char *request_json) {
    if (!shell->engine || !shell->profile || !shell->engine->browser_control_json) return NULL;
    UbarBytes request = {(const uint8_t *)request_json, strlen(request_json)};
    UbarOwnedBytes response = {0};
    if (shell->engine->browser_control_json(shell->profile, request, &response) != UBAR_OK ||
        !response.data) return NULL;
    char *copy = g_strndup((const char *)response.data, response.len);
    shell->engine->free_bytes(response);
    return copy;
}

static gboolean report_memory(gpointer data) {
    Shell *shell = data;
    if (!shell->engine || !shell->profile) return G_SOURCE_REMOVE;
    char *response = browser_control(shell,
        "{\"method\":\"reportMemory\",\"residentBytes\":0}");
    g_free(response);
    return G_SOURCE_CONTINUE;
}

static void on_bookmark(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    Tab *tab = current_tab(shell);
    const char *url = gtk_entry_get_text(GTK_ENTRY(shell->address));
    if (shell->private_mode || !tab || !url || (!g_str_has_prefix(url, "https://") &&
        !g_str_has_prefix(url, "http://"))) return;
    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "method"); json_builder_add_string_value(builder, "addBookmark");
    json_builder_set_member_name(builder, "title"); json_builder_add_string_value(builder, gtk_label_get_text(GTK_LABEL(tab->label)));
    json_builder_set_member_name(builder, "url"); json_builder_add_string_value(builder, url);
    json_builder_set_member_name(builder, "folder"); json_builder_add_string_value(builder, "");
    json_builder_set_member_name(builder, "createdAtMs"); json_builder_add_int_value(builder, g_get_real_time() / 1000);
    json_builder_end_object(builder);
    JsonGenerator *generator = json_generator_new();
    JsonNode *root = json_builder_get_root(builder);
    json_generator_set_root(generator, root);
    char *request = json_generator_to_data(generator, NULL);
    char *response = browser_control(shell, request);
    g_free(response); g_free(request); json_node_free(root);
    g_object_unref(generator); g_object_unref(builder);
}

static void on_library(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    char *bookmarks = browser_control(shell, "{\"method\":\"listBookmarks\"}");
    char *history = browser_control(shell, "{\"method\":\"queryHistory\",\"limit\":20}");
    char *downloads = browser_control(shell, "{\"method\":\"listDownloads\"}");
    char *summary = g_strdup_printf("Bookmarks\n%s\n\nRecent history\n%s\n\nDownloads\n%s",
        bookmarks ? bookmarks : "[]", history ? history : "[]", downloads ? downloads : "[]");
    GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(shell->window), GTK_DIALOG_MODAL,
        GTK_MESSAGE_INFO, GTK_BUTTONS_CLOSE, "%s", summary);
    gtk_window_set_title(GTK_WINDOW(dialog), "Library");
    gtk_dialog_run(GTK_DIALOG(dialog));
    gtk_widget_destroy(dialog);
    g_free(summary); g_free(bookmarks); g_free(history); g_free(downloads);
}

static void on_extension_action(GtkButton *button, gpointer data) {
    Shell *shell = data;
    const char *id = g_object_get_data(G_OBJECT(button), "extension-id");
    JsonBuilder *builder = json_builder_new();
    json_builder_begin_object(builder);
    json_builder_set_member_name(builder, "operation");
    json_builder_add_string_value(builder, "getAction");
    json_builder_set_member_name(builder, "id");
    json_builder_add_string_value(builder, id);
    json_builder_end_object(builder);
    JsonGenerator *generator = json_generator_new();
    JsonNode *root = json_builder_get_root(builder);
    json_generator_set_root(generator, root);
    char *request = json_generator_to_data(generator, NULL);
    char *response = extension_control(shell, request);
    const char *message = NULL;
    JsonParser *parser = json_parser_new();
    if (response && json_parser_load_from_data(parser, response, -1, NULL)) {
        JsonObject *object = json_node_get_object(json_parser_get_root(parser));
        if (json_object_has_member(object, "popup") && !json_object_get_null_member(object, "popup"))
            message = json_object_get_string_member(object, "popup");
    }
    Tab *tab = current_tab(shell);
    if (message && tab && tab->view) {
        UbarBytes uri = {(const uint8_t *)message, strlen(message)};
        shell->engine->navigate(tab->view, uri);
        gtk_entry_set_text(GTK_ENTRY(shell->address), message);
    } else {
        GtkWidget *dialog = gtk_message_dialog_new(GTK_WINDOW(shell->window), GTK_DIALOG_MODAL,
            GTK_MESSAGE_INFO, GTK_BUTTONS_CLOSE, "%s", "Extension action has no popup.");
        gtk_dialog_run(GTK_DIALOG(dialog));
        gtk_widget_destroy(dialog);
    }
    g_object_unref(parser);
    g_free(response);
    g_free(request);
    json_node_free(root);
    g_object_unref(generator);
    g_object_unref(builder);
}

static void populate_extension_actions(Shell *shell) {
    GList *children = gtk_container_get_children(GTK_CONTAINER(shell->extension_actions));
    for (GList *item = children; item; item = item->next)
        gtk_widget_destroy(GTK_WIDGET(item->data));
    g_list_free(children);
    char *response = extension_control(shell, "{\"operation\":\"list\"}");
    if (!response) return;
    JsonParser *parser = json_parser_new();
    if (json_parser_load_from_data(parser, response, -1, NULL) &&
        JSON_NODE_HOLDS_ARRAY(json_parser_get_root(parser))) {
        JsonArray *items = json_node_get_array(json_parser_get_root(parser));
        for (guint i = 0; i < json_array_get_length(items); ++i) {
            JsonObject *item = json_array_get_object_element(items, i);
            if (!json_object_has_member(item, "action") || json_object_get_null_member(item, "action")) continue;
            JsonObject *action = json_object_get_object_member(item, "action");
            const char *name = json_object_get_string_member(item, "name");
            const char *title = json_object_has_member(action, "default_title") &&
                !json_object_get_null_member(action, "default_title")
                ? json_object_get_string_member(action, "default_title") : name;
            GtkWidget *button = gtk_button_new_with_label(title);
            g_object_set_data_full(G_OBJECT(button), "extension-id",
                g_strdup(json_object_get_string_member(item, "id")), g_free);
            g_signal_connect(button, "clicked", G_CALLBACK(on_extension_action), shell);
            gtk_box_pack_start(GTK_BOX(shell->extension_actions), button, FALSE, FALSE, 0);
            gtk_widget_show(button);
        }
    }
    g_object_unref(parser);
    g_free(response);
}

static void on_install_extension(GtkButton *button, gpointer data) {
    (void)button;
    Shell *shell = data;
    GtkWidget *chooser = gtk_file_chooser_dialog_new(
        "Install signed browser extension", GTK_WINDOW(shell->window),
        GTK_FILE_CHOOSER_ACTION_OPEN, "Cancel", GTK_RESPONSE_CANCEL,
        "Install", GTK_RESPONSE_ACCEPT, NULL);
    GtkFileFilter *filter = gtk_file_filter_new();
    gtk_file_filter_set_name(filter, "Browser extensions (*.xpi, *.crx)");
    gtk_file_filter_add_pattern(filter, "*.xpi");
    gtk_file_filter_add_pattern(filter, "*.crx");
    gtk_file_chooser_add_filter(GTK_FILE_CHOOSER(chooser), filter);
    if (gtk_dialog_run(GTK_DIALOG(chooser)) == GTK_RESPONSE_ACCEPT) {
        char *path = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(chooser));
        JsonBuilder *builder = json_builder_new();
        json_builder_begin_object(builder);
        json_builder_set_member_name(builder, "operation");
        json_builder_add_string_value(builder, "installPackage");
        json_builder_set_member_name(builder, "path");
        json_builder_add_string_value(builder, path);
        json_builder_end_object(builder);
        JsonGenerator *generator = json_generator_new();
        JsonNode *root = json_builder_get_root(builder);
        json_generator_set_root(generator, root);
        char *request = json_generator_to_data(generator, NULL);
        char *response = extension_control(shell, request);
        if (response) {
            populate_extension_actions(shell);
        } else {
            GtkWidget *error = gtk_message_dialog_new(GTK_WINDOW(shell->window), GTK_DIALOG_MODAL,
                GTK_MESSAGE_ERROR, GTK_BUTTONS_CLOSE,
                "%s", "Package signature, integrity, or compatibility validation failed.");
            gtk_dialog_run(GTK_DIALOG(error));
            gtk_widget_destroy(error);
        }
        g_free(response);
        g_free(request);
        json_node_free(root);
        g_object_unref(generator);
        g_object_unref(builder);
        g_free(path);
    }
    gtk_widget_destroy(chooser);
}

static void activate(GtkApplication *application, gpointer data) {
    Shell *shell = data;
    shell->tabs = g_ptr_array_new();
    load_engine(shell);
    shell->window = gtk_application_window_new(application);
    gtk_window_set_default_size(GTK_WINDOW(shell->window), 1100, 720);
    gtk_window_set_title(GTK_WINDOW(shell->window), shell->private_mode ? "uBar Private" : "uBar");

    GtkWidget *root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    GtkWidget *toolbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    gtk_widget_set_name(toolbar, "floating-toolbar");
    GtkWidget *back = gtk_button_new_from_icon_name("go-previous-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *forward = gtk_button_new_from_icon_name("go-next-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *reload = gtk_button_new_from_icon_name("view-refresh-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *stop = gtk_button_new_from_icon_name("process-stop-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *zoom_out = gtk_button_new_with_label("−");
    GtkWidget *zoom_in = gtk_button_new_with_label("+");
    GtkWidget *close = gtk_button_new_from_icon_name("window-close-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *add = gtk_button_new_from_icon_name("list-add-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *install_extension = gtk_button_new_with_label("Extensions +");
    GtkWidget *bookmark = gtk_button_new_with_label("★");
    GtkWidget *library = gtk_button_new_with_label("Library");
    shell->address = gtk_entry_new();
    shell->extension_actions = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 4);
    gtk_entry_set_placeholder_text(GTK_ENTRY(shell->address), "Search or enter address");
    gtk_box_pack_start(GTK_BOX(toolbar), back, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), forward, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), reload, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), stop, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), shell->address, TRUE, TRUE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), shell->extension_actions, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), bookmark, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), library, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), install_extension, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), zoom_out, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), zoom_in, FALSE, FALSE, 0);
    if (shell->private_mode) {
        gtk_box_pack_start(GTK_BOX(toolbar), gtk_label_new("Private"), FALSE, FALSE, 0);
    } else {
        GtkWidget *private_button = gtk_button_new_with_label("Private");
        gtk_box_pack_start(GTK_BOX(toolbar), private_button, FALSE, FALSE, 0);
        g_signal_connect(private_button, "clicked", G_CALLBACK(open_private), shell);
    }
    gtk_box_pack_start(GTK_BOX(toolbar), add, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), close, FALSE, FALSE, 0);
    shell->notebook = gtk_notebook_new();
    gtk_widget_set_name(shell->notebook, "browser-tabs");
    gtk_notebook_set_scrollable(GTK_NOTEBOOK(shell->notebook), TRUE);
    gtk_box_pack_start(GTK_BOX(root), toolbar, FALSE, FALSE, 8);
    gtk_box_pack_start(GTK_BOX(root), shell->notebook, TRUE, TRUE, 0);
    gtk_container_add(GTK_CONTAINER(shell->window), root);
    GtkCssProvider *styles = gtk_css_provider_new();
    gtk_css_provider_load_from_data(styles,
        "#floating-toolbar { margin: 12px; padding: 8px; border-radius: 18px; "
        "background-color: alpha(@theme_bg_color, 0.94); box-shadow: 0 6px 20px alpha(black, 0.18); }"
        "#browser-tabs { margin: 0 8px 8px 8px; border-radius: 14px; }"
        "button { min-height: 28px; border-radius: 10px; }", -1, NULL);
    gtk_style_context_add_provider_for_screen(gdk_screen_get_default(),
        GTK_STYLE_PROVIDER(styles), GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
    g_object_unref(styles);
    gtk_widget_show_all(shell->window);

    if (shell->engine) {
        uint64_t memory = physical_memory();
        UbarProfileConfigV1 profile = {
            .struct_size = sizeof(profile),
            .kind = shell->private_mode ? UBAR_PROFILE_PRIVATE : UBAR_PROFILE_NORMAL,
            .memory_target_bytes = memory / 4,
            .memory_ceiling_bytes = memory * 3 / 4,
            .partition_third_party_storage = true,
            .block_third_party_cookies = true,
            .require_sandbox = true,
        };
        shell->engine->create_profile(&profile, &shell->profile);
        if (shell->profile && !shell->memory_timer)
            shell->memory_timer = g_timeout_add_seconds(5, report_memory, shell);
        populate_extension_actions(shell);
    }

    g_signal_connect(add, "clicked", G_CALLBACK(on_add_tab), shell);
    g_signal_connect(install_extension, "clicked", G_CALLBACK(on_install_extension), shell);
    g_signal_connect(bookmark, "clicked", G_CALLBACK(on_bookmark), shell);
    g_signal_connect(library, "clicked", G_CALLBACK(on_library), shell);
    g_signal_connect(back, "clicked", G_CALLBACK(on_back), shell);
    g_signal_connect(forward, "clicked", G_CALLBACK(on_forward), shell);
    g_signal_connect(reload, "clicked", G_CALLBACK(on_reload), shell);
    g_signal_connect(stop, "clicked", G_CALLBACK(on_stop), shell);
    g_signal_connect(zoom_out, "clicked", G_CALLBACK(on_zoom_out), shell);
    g_signal_connect(zoom_in, "clicked", G_CALLBACK(on_zoom_in), shell);
    g_signal_connect(close, "clicked", G_CALLBACK(on_close_tab), shell);
    g_signal_connect(shell->address, "activate", G_CALLBACK(on_address), shell);
    g_signal_connect(shell->notebook, "switch-page", G_CALLBACK(on_switch_page), shell);
    add_tab(shell);
}

int main(int argc, char **argv) {
    Shell shell = {0};
    for (int i = 1; i < argc; ++i)
        if (strcmp(argv[i], "--incognito") == 0) shell.private_mode = true;
    GtkApplication *application = gtk_application_new("dev.ghanti.ubar", G_APPLICATION_FLAGS_NONE);
    g_signal_connect(application, "activate", G_CALLBACK(activate), &shell);
    int status = g_application_run(G_APPLICATION(application), argc, argv);
    if (shell.memory_timer) g_source_remove(shell.memory_timer);
    if (shell.engine && shell.tabs) {
        for (guint i = 0; i < shell.tabs->len; ++i) {
            Tab *tab = g_ptr_array_index(shell.tabs, i);
            if (tab->view) shell.engine->destroy_view(tab->view);
            g_free(tab->uri);
            g_free(tab);
        }
    }
    if (shell.tabs) g_ptr_array_free(shell.tabs, TRUE);
    if (shell.engine && shell.profile) shell.engine->destroy_profile(shell.profile);
    if (shell.engine_library) dlclose(shell.engine_library);
    g_object_unref(application);
    return status;
}
