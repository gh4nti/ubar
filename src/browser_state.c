#include "browser_state.h"

#include <glib/gstdio.h>
#include <string.h>

struct BrowserState {
    char *data_dir;
    char *state_path;
    char *backup_state_path;
    char *homepage_uri;
    char *notifications_default;
    char *location_default;
    char *cookies_default;
    char *camera_default;
    char *microphone_default;
    char *javascript_default;
    char *popups_default;
    GPtrArray *history;
    GPtrArray *bookmarks;
    GPtrArray *site_permissions;
};

static void browser_state_save(BrowserState *state);

static void
history_entry_free(gpointer data)
{
    BrowserHistoryEntry *entry = data;
    g_free(entry->title);
    g_free(entry->uri);
    g_free(entry);
}

static void
bookmark_entry_free(gpointer data)
{
    BrowserBookmarkEntry *entry = data;
    g_free(entry->title);
    g_free(entry->uri);
    g_free(entry);
}

static void
site_permission_entry_free(gpointer data)
{
    BrowserSitePermissionEntry *entry = data;
    g_free(entry->origin);
    g_free(entry->notifications);
    g_free(entry->location);
    g_free(entry->cookies);
    g_free(entry->camera);
    g_free(entry->microphone);
    g_free(entry->javascript);
    g_free(entry->popups);
    g_free(entry);
}

static gboolean
is_permission_value(const char *value)
{
    return g_strcmp0(value, "ask") == 0 ||
           g_strcmp0(value, "allow") == 0 ||
           g_strcmp0(value, "block") == 0;
}

static const char *
clean_permission_value(const char *value, const char *fallback)
{
    return is_permission_value(value) ? value : fallback;
}

static char **
default_permission_slot(BrowserState *state, const char *permission)
{
    if (g_strcmp0(permission, "notifications") == 0) {
        return &state->notifications_default;
    }
    if (g_strcmp0(permission, "location") == 0) {
        return &state->location_default;
    }
    if (g_strcmp0(permission, "cookies") == 0) {
        return &state->cookies_default;
    }
    if (g_strcmp0(permission, "camera") == 0) {
        return &state->camera_default;
    }
    if (g_strcmp0(permission, "microphone") == 0) {
        return &state->microphone_default;
    }
    if (g_strcmp0(permission, "javascript") == 0) {
        return &state->javascript_default;
    }
    if (g_strcmp0(permission, "popups") == 0) {
        return &state->popups_default;
    }
    return NULL;
}

static char **
site_permission_slot(BrowserSitePermissionEntry *entry, const char *permission)
{
    if (g_strcmp0(permission, "notifications") == 0) {
        return &entry->notifications;
    }
    if (g_strcmp0(permission, "location") == 0) {
        return &entry->location;
    }
    if (g_strcmp0(permission, "cookies") == 0) {
        return &entry->cookies;
    }
    if (g_strcmp0(permission, "camera") == 0) {
        return &entry->camera;
    }
    if (g_strcmp0(permission, "microphone") == 0) {
        return &entry->microphone;
    }
    if (g_strcmp0(permission, "javascript") == 0) {
        return &entry->javascript;
    }
    if (g_strcmp0(permission, "popups") == 0) {
        return &entry->popups;
    }
    return NULL;
}

static char *
key_file_get_string_default(GKeyFile *key_file, const char *group, const char *key, const char *fallback)
{
    char *value = g_key_file_get_string(key_file, group, key, NULL);
    if (value == NULL || !is_permission_value(value)) {
        g_free(value);
        return g_strdup(fallback);
    }
    return value;
}

static void
browser_state_save(BrowserState *state)
{
    GKeyFile *key_file;
    gsize length;
    gchar *data;
    char *tmp_path;
    GError *error;
    guint i;

    key_file = g_key_file_new();
    g_key_file_set_string(key_file, "Settings", "homepage_uri", state->homepage_uri);
    g_key_file_set_string(key_file, "Settings", "notifications_default", state->notifications_default);
    g_key_file_set_string(key_file, "Settings", "location_default", state->location_default);
    g_key_file_set_string(key_file, "Settings", "cookies_default", state->cookies_default);
    g_key_file_set_string(key_file, "Settings", "camera_default", state->camera_default);
    g_key_file_set_string(key_file, "Settings", "microphone_default", state->microphone_default);
    g_key_file_set_string(key_file, "Settings", "javascript_default", state->javascript_default);
    g_key_file_set_string(key_file, "Settings", "popups_default", state->popups_default);
    g_key_file_set_integer(key_file, "Settings", "history_count", (gint)state->history->len);
    g_key_file_set_integer(key_file, "Settings", "bookmark_count", (gint)state->bookmarks->len);
    g_key_file_set_integer(key_file, "Settings", "site_permission_count", (gint)state->site_permissions->len);

    for (i = 0; i < state->history->len; i++) {
        BrowserHistoryEntry *entry = g_ptr_array_index(state->history, i);
        char *group = g_strdup_printf("History %u", i);
        g_key_file_set_string(key_file, group, "title", entry->title);
        g_key_file_set_string(key_file, group, "uri", entry->uri);
        g_key_file_set_int64(key_file, group, "timestamp", entry->timestamp);
        g_free(group);
    }

    for (i = 0; i < state->bookmarks->len; i++) {
        BrowserBookmarkEntry *entry = g_ptr_array_index(state->bookmarks, i);
        char *group = g_strdup_printf("Bookmark %u", i);
        g_key_file_set_string(key_file, group, "title", entry->title);
        g_key_file_set_string(key_file, group, "uri", entry->uri);
        g_key_file_set_int64(key_file, group, "timestamp", entry->timestamp);
        g_free(group);
    }

    for (i = 0; i < state->site_permissions->len; i++) {
        BrowserSitePermissionEntry *entry = g_ptr_array_index(state->site_permissions, i);
        char *group = g_strdup_printf("Site Permission %u", i);
        g_key_file_set_string(key_file, group, "origin", entry->origin);
        g_key_file_set_string(key_file, group, "notifications", entry->notifications);
        g_key_file_set_string(key_file, group, "location", entry->location);
        g_key_file_set_string(key_file, group, "cookies", entry->cookies);
        g_key_file_set_string(key_file, group, "camera", entry->camera);
        g_key_file_set_string(key_file, group, "microphone", entry->microphone);
        g_key_file_set_string(key_file, group, "javascript", entry->javascript);
        g_key_file_set_string(key_file, group, "popups", entry->popups);
        g_free(group);
    }

    data = g_key_file_to_data(key_file, &length, NULL);
    tmp_path = g_strdup_printf("%s.tmp", state->state_path);
    error = NULL;

    if (g_file_set_contents(tmp_path, data, (gssize)length, &error)) {
        if (g_file_test(state->state_path, G_FILE_TEST_EXISTS)) {
            g_rename(state->state_path, state->backup_state_path);
        }

        if (g_rename(tmp_path, state->state_path) != 0) {
            g_rename(state->backup_state_path, state->state_path);
            g_remove(tmp_path);
        }
    } else {
        g_clear_error(&error);
    }

    g_free(tmp_path);
    g_free(data);
    g_key_file_unref(key_file);
}

static BrowserHistoryEntry *
history_entry_new(const char *title, const char *uri, gint64 timestamp)
{
    BrowserHistoryEntry *entry = g_new0(BrowserHistoryEntry, 1);
    entry->title = g_strdup(title);
    entry->uri = g_strdup(uri);
    entry->timestamp = timestamp;
    return entry;
}

static BrowserBookmarkEntry *
bookmark_entry_new(const char *title, const char *uri, gint64 timestamp)
{
    BrowserBookmarkEntry *entry = g_new0(BrowserBookmarkEntry, 1);
    entry->title = g_strdup(title);
    entry->uri = g_strdup(uri);
    entry->timestamp = timestamp;
    return entry;
}

static BrowserSitePermissionEntry *
site_permission_entry_new(const char *origin)
{
    BrowserSitePermissionEntry *entry = g_new0(BrowserSitePermissionEntry, 1);
    entry->origin = g_strdup(origin);
    entry->notifications = g_strdup("ask");
    entry->location = g_strdup("ask");
    entry->cookies = g_strdup("allow");
    entry->camera = g_strdup("ask");
    entry->microphone = g_strdup("ask");
    entry->javascript = g_strdup("allow");
    entry->popups = g_strdup("block");
    return entry;
}

BrowserState *
browser_state_new(void)
{
    BrowserState *state;
    GKeyFile *key_file;
    gint history_count;
    gint bookmark_count;
    gint site_permission_count;
    gint i;

    state = g_new0(BrowserState, 1);
    state->data_dir = g_build_filename(g_get_user_data_dir(), "ubar", NULL);
    state->state_path = g_build_filename(state->data_dir, "state.ini", NULL);
    state->backup_state_path = g_build_filename(state->data_dir, "state.ini.bak", NULL);
    state->homepage_uri = g_strdup("");
    state->notifications_default = g_strdup("ask");
    state->location_default = g_strdup("ask");
    state->cookies_default = g_strdup("allow");
    state->camera_default = g_strdup("ask");
    state->microphone_default = g_strdup("ask");
    state->javascript_default = g_strdup("allow");
    state->popups_default = g_strdup("block");
    state->history = g_ptr_array_new_with_free_func(history_entry_free);
    state->bookmarks = g_ptr_array_new_with_free_func(bookmark_entry_free);
    state->site_permissions = g_ptr_array_new_with_free_func(site_permission_entry_free);

    g_mkdir_with_parents(state->data_dir, 0755);

    key_file = g_key_file_new();
    if (!g_key_file_load_from_file(key_file, state->state_path, G_KEY_FILE_NONE, NULL) &&
        !g_key_file_load_from_file(key_file, state->backup_state_path, G_KEY_FILE_NONE, NULL)) {
        g_key_file_unref(key_file);
        return state;
    }

    g_free(state->homepage_uri);
    state->homepage_uri = g_key_file_get_string(key_file, "Settings", "homepage_uri", NULL);
    if (state->homepage_uri == NULL) {
        state->homepage_uri = g_strdup("");
    }

    g_free(state->notifications_default);
    g_free(state->location_default);
    g_free(state->cookies_default);
    g_free(state->camera_default);
    g_free(state->microphone_default);
    g_free(state->javascript_default);
    g_free(state->popups_default);
    state->notifications_default = key_file_get_string_default(key_file, "Settings", "notifications_default", "ask");
    state->location_default = key_file_get_string_default(key_file, "Settings", "location_default", "ask");
    state->cookies_default = key_file_get_string_default(key_file, "Settings", "cookies_default", "allow");
    state->camera_default = key_file_get_string_default(key_file, "Settings", "camera_default", "ask");
    state->microphone_default = key_file_get_string_default(key_file, "Settings", "microphone_default", "ask");
    state->javascript_default = key_file_get_string_default(key_file, "Settings", "javascript_default", "allow");
    state->popups_default = key_file_get_string_default(key_file, "Settings", "popups_default", "block");

    history_count = g_key_file_get_integer(key_file, "Settings", "history_count", NULL);
    bookmark_count = g_key_file_get_integer(key_file, "Settings", "bookmark_count", NULL);
    site_permission_count = g_key_file_get_integer(key_file, "Settings", "site_permission_count", NULL);

    for (i = 0; i < history_count; i++) {
        BrowserHistoryEntry *entry;
        char *group = g_strdup_printf("History %d", i);
        char *title = g_key_file_get_string(key_file, group, "title", NULL);
        char *uri = g_key_file_get_string(key_file, group, "uri", NULL);
        if (title != NULL && uri != NULL) {
            entry = history_entry_new(title, uri, g_key_file_get_int64(key_file, group, "timestamp", NULL));
            g_ptr_array_add(state->history, entry);
        }
        g_free(title);
        g_free(uri);
        g_free(group);
    }

    for (i = 0; i < bookmark_count; i++) {
        BrowserBookmarkEntry *entry;
        char *group = g_strdup_printf("Bookmark %d", i);
        char *title = g_key_file_get_string(key_file, group, "title", NULL);
        char *uri = g_key_file_get_string(key_file, group, "uri", NULL);
        if (title != NULL && uri != NULL) {
            entry = bookmark_entry_new(title, uri, g_key_file_get_int64(key_file, group, "timestamp", NULL));
            g_ptr_array_add(state->bookmarks, entry);
        }
        g_free(title);
        g_free(uri);
        g_free(group);
    }

    for (i = 0; i < site_permission_count; i++) {
        BrowserSitePermissionEntry *entry;
        char *group = g_strdup_printf("Site Permission %d", i);
        char *origin = g_key_file_get_string(key_file, group, "origin", NULL);
        if (origin != NULL && *origin != '\0') {
            entry = site_permission_entry_new(origin);
            g_free(entry->notifications);
            g_free(entry->location);
            g_free(entry->cookies);
            g_free(entry->camera);
            g_free(entry->microphone);
            g_free(entry->javascript);
            g_free(entry->popups);
            entry->notifications = key_file_get_string_default(key_file, group, "notifications", "ask");
            entry->location = key_file_get_string_default(key_file, group, "location", "ask");
            entry->cookies = key_file_get_string_default(key_file, group, "cookies", "allow");
            entry->camera = key_file_get_string_default(key_file, group, "camera", "ask");
            entry->microphone = key_file_get_string_default(key_file, group, "microphone", "ask");
            entry->javascript = key_file_get_string_default(key_file, group, "javascript", "allow");
            entry->popups = key_file_get_string_default(key_file, group, "popups", "block");
            g_ptr_array_add(state->site_permissions, entry);
        }
        g_free(origin);
        g_free(group);
    }

    g_key_file_unref(key_file);
    return state;
}

void
browser_state_free(BrowserState *state)
{
    if (state == NULL) {
        return;
    }

    g_free(state->data_dir);
    g_free(state->state_path);
    g_free(state->backup_state_path);
    g_free(state->homepage_uri);
    g_free(state->notifications_default);
    g_free(state->location_default);
    g_free(state->cookies_default);
    g_free(state->camera_default);
    g_free(state->microphone_default);
    g_free(state->javascript_default);
    g_free(state->popups_default);
    g_ptr_array_unref(state->history);
    g_ptr_array_unref(state->bookmarks);
    g_ptr_array_unref(state->site_permissions);
    g_free(state);
}

const char *
browser_state_get_homepage_uri(BrowserState *state)
{
    return state->homepage_uri;
}

void
browser_state_set_homepage_uri(BrowserState *state, const char *uri)
{
    g_free(state->homepage_uri);
    state->homepage_uri = g_strdup(uri != NULL ? uri : "");
    browser_state_save(state);
}

GPtrArray *
browser_state_get_history(BrowserState *state)
{
    return state->history;
}

GPtrArray *
browser_state_get_bookmarks(BrowserState *state)
{
    return state->bookmarks;
}

void
browser_state_add_history(BrowserState *state, const char *title, const char *uri)
{
    BrowserHistoryEntry *entry;

    if (uri == NULL || *uri == '\0') {
        return;
    }

    entry = history_entry_new(title != NULL && *title != '\0' ? title : uri,
                              uri,
                              g_get_real_time() / G_USEC_PER_SEC);
    g_ptr_array_insert(state->history, 0, entry);
    browser_state_save(state);
}

void
browser_state_clear_history(BrowserState *state)
{
    g_ptr_array_set_size(state->history, 0);
    browser_state_save(state);
}

gboolean
browser_state_is_bookmarked(BrowserState *state, const char *uri)
{
    guint i;

    if (uri == NULL || *uri == '\0') {
        return FALSE;
    }

    for (i = 0; i < state->bookmarks->len; i++) {
        BrowserBookmarkEntry *entry = g_ptr_array_index(state->bookmarks, i);
        if (g_strcmp0(entry->uri, uri) == 0) {
            return TRUE;
        }
    }

    return FALSE;
}

gboolean
browser_state_remove_bookmark(BrowserState *state, const char *uri)
{
    guint i;

    for (i = 0; i < state->bookmarks->len; i++) {
        BrowserBookmarkEntry *entry = g_ptr_array_index(state->bookmarks, i);
        if (g_strcmp0(entry->uri, uri) == 0) {
            g_ptr_array_remove_index(state->bookmarks, i);
            browser_state_save(state);
            return TRUE;
        }
    }

    return FALSE;
}

gboolean
browser_state_toggle_bookmark(BrowserState *state, const char *title, const char *uri)
{
    BrowserBookmarkEntry *entry;

    if (uri == NULL || *uri == '\0') {
        return FALSE;
    }

    if (browser_state_remove_bookmark(state, uri)) {
        return FALSE;
    }

    entry = bookmark_entry_new(title != NULL && *title != '\0' ? title : uri,
                               uri,
                               g_get_real_time() / G_USEC_PER_SEC);
    g_ptr_array_insert(state->bookmarks, 0, entry);
    browser_state_save(state);
    return TRUE;
}

const char *
browser_state_get_default_permission(BrowserState *state, const char *permission)
{
    char **slot = default_permission_slot(state, permission);
    return slot != NULL && *slot != NULL ? *slot : "ask";
}

void
browser_state_set_default_permission(BrowserState *state, const char *permission, const char *value)
{
    char **slot = default_permission_slot(state, permission);
    if (slot == NULL) {
        return;
    }

    g_free(*slot);
    *slot = g_strdup(clean_permission_value(value, "ask"));
    browser_state_save(state);
}

GPtrArray *
browser_state_get_site_permissions(BrowserState *state)
{
    return state->site_permissions;
}

BrowserSitePermissionEntry *
browser_state_get_site_permission(BrowserState *state, const char *origin)
{
    guint i;

    if (origin == NULL || *origin == '\0') {
        return NULL;
    }

    for (i = 0; i < state->site_permissions->len; i++) {
        BrowserSitePermissionEntry *entry = g_ptr_array_index(state->site_permissions, i);
        if (g_strcmp0(entry->origin, origin) == 0) {
            return entry;
        }
    }

    return NULL;
}

const char *
browser_state_get_effective_site_permission(BrowserState *state, const char *origin, const char *permission)
{
    BrowserSitePermissionEntry *entry;
    char **slot;

    entry = browser_state_get_site_permission(state, origin);
    if (entry != NULL) {
        slot = site_permission_slot(entry, permission);
        if (slot != NULL && *slot != NULL) {
            return *slot;
        }
    }

    return browser_state_get_default_permission(state, permission);
}

void
browser_state_set_site_permission(BrowserState *state, const char *origin, const char *permission, const char *value)
{
    BrowserSitePermissionEntry *entry;
    char **slot;

    if (origin == NULL || *origin == '\0') {
        return;
    }

    entry = browser_state_get_site_permission(state, origin);
    if (entry == NULL) {
        entry = site_permission_entry_new(origin);
        g_ptr_array_insert(state->site_permissions, 0, entry);
    }

    slot = site_permission_slot(entry, permission);
    if (slot == NULL) {
        return;
    }

    g_free(*slot);
    *slot = g_strdup(clean_permission_value(value, browser_state_get_default_permission(state, permission)));
    browser_state_save(state);
}

gboolean
browser_state_remove_site_permissions(BrowserState *state, const char *origin)
{
    guint i;

    if (origin == NULL || *origin == '\0') {
        return FALSE;
    }

    for (i = 0; i < state->site_permissions->len; i++) {
        BrowserSitePermissionEntry *entry = g_ptr_array_index(state->site_permissions, i);
        if (g_strcmp0(entry->origin, origin) == 0) {
            g_ptr_array_remove_index(state->site_permissions, i);
            browser_state_save(state);
            return TRUE;
        }
    }

    return FALSE;
}
