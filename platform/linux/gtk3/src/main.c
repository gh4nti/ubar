#include <dlfcn.h>
#include <gtk/gtk.h>
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
} Tab;

struct Shell {
    GtkWidget *window;
    GtkWidget *notebook;
    GtkWidget *address;
    GPtrArray *tabs;
    void *engine_library;
    const UbarEngineApiV1 *engine;
    UbarProfile profile;
    bool private_mode;
};

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
        gtk_entry_set_text(GTK_ENTRY(tab->shell->address), text);
    } else if (event->kind == UBAR_RENDERER_CRASHED) {
        gtk_label_set_text(GTK_LABEL(tab->label), "Crashed");
    }
    g_free(text);
}

static void add_tab(Shell *shell) {
    Tab *tab = g_new0(Tab, 1);
    tab->shell = shell;
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
    char *uri = strstr(text, "://") ? g_strdup(text) : g_strdup_printf("https://%s", text);
    UbarBytes bytes = {(const uint8_t *)uri, strlen(uri)};
    shell->engine->navigate(tab->view, bytes);
    g_free(uri);
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

static void activate(GtkApplication *application, gpointer data) {
    Shell *shell = data;
    shell->tabs = g_ptr_array_new();
    load_engine(shell);
    shell->window = gtk_application_window_new(application);
    gtk_window_set_default_size(GTK_WINDOW(shell->window), 1100, 720);
    gtk_window_set_title(GTK_WINDOW(shell->window), shell->private_mode ? "uBar Private" : "uBar");

    GtkWidget *root = gtk_box_new(GTK_ORIENTATION_VERTICAL, 8);
    GtkWidget *toolbar = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 6);
    GtkWidget *back = gtk_button_new_from_icon_name("go-previous-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *forward = gtk_button_new_from_icon_name("go-next-symbolic", GTK_ICON_SIZE_BUTTON);
    GtkWidget *add = gtk_button_new_from_icon_name("list-add-symbolic", GTK_ICON_SIZE_BUTTON);
    shell->address = gtk_entry_new();
    gtk_entry_set_placeholder_text(GTK_ENTRY(shell->address), "Search or enter address");
    gtk_box_pack_start(GTK_BOX(toolbar), back, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), forward, FALSE, FALSE, 0);
    gtk_box_pack_start(GTK_BOX(toolbar), shell->address, TRUE, TRUE, 0);
    if (shell->private_mode) {
        gtk_box_pack_start(GTK_BOX(toolbar), gtk_label_new("Private"), FALSE, FALSE, 0);
    } else {
        GtkWidget *private_button = gtk_button_new_with_label("Private");
        gtk_box_pack_start(GTK_BOX(toolbar), private_button, FALSE, FALSE, 0);
        g_signal_connect(private_button, "clicked", G_CALLBACK(open_private), shell);
    }
    gtk_box_pack_start(GTK_BOX(toolbar), add, FALSE, FALSE, 0);
    shell->notebook = gtk_notebook_new();
    gtk_notebook_set_scrollable(GTK_NOTEBOOK(shell->notebook), TRUE);
    gtk_box_pack_start(GTK_BOX(root), toolbar, FALSE, FALSE, 8);
    gtk_box_pack_start(GTK_BOX(root), shell->notebook, TRUE, TRUE, 0);
    gtk_container_add(GTK_CONTAINER(shell->window), root);
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
    }

    g_signal_connect(add, "clicked", G_CALLBACK(on_add_tab), shell);
    g_signal_connect(back, "clicked", G_CALLBACK(on_back), shell);
    g_signal_connect(forward, "clicked", G_CALLBACK(on_forward), shell);
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
    if (shell.engine && shell.tabs) {
        for (guint i = 0; i < shell.tabs->len; ++i) {
            Tab *tab = g_ptr_array_index(shell.tabs, i);
            if (tab->view) shell.engine->destroy_view(tab->view);
            g_free(tab);
        }
    }
    if (shell.tabs) g_ptr_array_free(shell.tabs, TRUE);
    if (shell.engine && shell.profile) shell.engine->destroy_profile(shell.profile);
    if (shell.engine_library) dlclose(shell.engine_library);
    g_object_unref(application);
    return status;
}
