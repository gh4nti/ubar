#include "browser_state.h"

#include <glib/gstdio.h>

struct BrowserState {
    char *data_dir;
    char *state_path;
    char *homepage_uri;
    GPtrArray *history;
    GPtrArray *bookmarks;
};

static void
history_entry_free(gpointer data)
{
    BrowserHistoryEntry *entry;

    entry = data;
    g_free(entry->title);
    g_free(entry->uri);
    g_free(entry);
}

static void
bookmark_entry_free(gpointer data)
{
    BrowserBookmarkEntry *entry;

    entry = data;
    g_free(entry->title);
    g_free(entry->uri);
    g_free(entry);
}

static void
browser_state_save(BrowserState *state)
{
    GKeyFile *key_file;
    gsize length;
    gchar *data;
    guint i;

    key_file = g_key_file_new();
    g_key_file_set_string(key_file, "Settings", "homepage_uri", state->homepage_uri);
    g_key_file_set_integer(key_file, "Settings", "history_count", (gint)state->history->len);
    g_key_file_set_integer(key_file, "Settings", "bookmark_count", (gint)state->bookmarks->len);

    for (i = 0; i < state->history->len; i++) {
        BrowserHistoryEntry *entry;
        char *group;

        entry = g_ptr_array_index(state->history, i);
        group = g_strdup_printf("History %u", i);
        g_key_file_set_string(key_file, group, "title", entry->title);
        g_key_file_set_string(key_file, group, "uri", entry->uri);
        g_key_file_set_int64(key_file, group, "timestamp", entry->timestamp);
        g_free(group);
    }

    for (i = 0; i < state->bookmarks->len; i++) {
        BrowserBookmarkEntry *entry;
        char *group;

        entry = g_ptr_array_index(state->bookmarks, i);
        group = g_strdup_printf("Bookmark %u", i);
        g_key_file_set_string(key_file, group, "title", entry->title);
        g_key_file_set_string(key_file, group, "uri", entry->uri);
        g_key_file_set_int64(key_file, group, "timestamp", entry->timestamp);
        g_free(group);
    }

    data = g_key_file_to_data(key_file, &length, NULL);
    g_file_set_contents(state->state_path, data, (gssize)length, NULL);

    g_free(data);
    g_key_file_unref(key_file);
}

static BrowserHistoryEntry *
history_entry_new(const char *title, const char *uri, gint64 timestamp)
{
    BrowserHistoryEntry *entry;

    entry = g_new0(BrowserHistoryEntry, 1);
    entry->title = g_strdup(title);
    entry->uri = g_strdup(uri);
    entry->timestamp = timestamp;
    return entry;
}

static BrowserBookmarkEntry *
bookmark_entry_new(const char *title, const char *uri, gint64 timestamp)
{
    BrowserBookmarkEntry *entry;

    entry = g_new0(BrowserBookmarkEntry, 1);
    entry->title = g_strdup(title);
    entry->uri = g_strdup(uri);
    entry->timestamp = timestamp;
    return entry;
}

BrowserState *
browser_state_new(void)
{
    BrowserState *state;
    GKeyFile *key_file;
    gint history_count;
    gint bookmark_count;
    gint i;

    state = g_new0(BrowserState, 1);
    state->data_dir = g_build_filename(g_get_user_data_dir(), "ubar", NULL);
    state->state_path = g_build_filename(state->data_dir, "state.ini", NULL);
    state->homepage_uri = g_strdup("");
    state->history = g_ptr_array_new_with_free_func(history_entry_free);
    state->bookmarks = g_ptr_array_new_with_free_func(bookmark_entry_free);

    g_mkdir_with_parents(state->data_dir, 0755);

    key_file = g_key_file_new();
    if (!g_key_file_load_from_file(key_file, state->state_path, G_KEY_FILE_NONE, NULL)) {
        g_key_file_unref(key_file);
        return state;
    }

    g_free(state->homepage_uri);
    state->homepage_uri = g_key_file_get_string(key_file, "Settings", "homepage_uri", NULL);
    if (state->homepage_uri == NULL) {
        state->homepage_uri = g_strdup("");
    }

    history_count = g_key_file_get_integer(key_file, "Settings", "history_count", NULL);
    bookmark_count = g_key_file_get_integer(key_file, "Settings", "bookmark_count", NULL);

    for (i = 0; i < history_count; i++) {
        BrowserHistoryEntry *entry;
        char *group;
        char *title;
        char *uri;

        group = g_strdup_printf("History %d", i);
        title = g_key_file_get_string(key_file, group, "title", NULL);
        uri = g_key_file_get_string(key_file, group, "uri", NULL);
        if (title != NULL && uri != NULL) {
            entry = history_entry_new(title,
                                      uri,
                                      g_key_file_get_int64(key_file, group, "timestamp", NULL));
            g_ptr_array_add(state->history, entry);
        }
        g_free(title);
        g_free(uri);
        g_free(group);
    }

    for (i = 0; i < bookmark_count; i++) {
        BrowserBookmarkEntry *entry;
        char *group;
        char *title;
        char *uri;

        group = g_strdup_printf("Bookmark %d", i);
        title = g_key_file_get_string(key_file, group, "title", NULL);
        uri = g_key_file_get_string(key_file, group, "uri", NULL);
        if (title != NULL && uri != NULL) {
            entry = bookmark_entry_new(title,
                                       uri,
                                       g_key_file_get_int64(key_file, group, "timestamp", NULL));
            g_ptr_array_add(state->bookmarks, entry);
        }
        g_free(title);
        g_free(uri);
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
    g_free(state->homepage_uri);
    g_ptr_array_unref(state->history);
    g_ptr_array_unref(state->bookmarks);
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

    while (state->history->len > 200) {
        g_ptr_array_remove_index(state->history, state->history->len - 1);
    }

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
        BrowserBookmarkEntry *entry;

        entry = g_ptr_array_index(state->bookmarks, i);
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
        BrowserBookmarkEntry *entry;

        entry = g_ptr_array_index(state->bookmarks, i);
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
