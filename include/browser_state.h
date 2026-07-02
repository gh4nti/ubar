#ifndef BROWSER_STATE_H
#define BROWSER_STATE_H

#include <glib.h>

typedef struct {
    char *title;
    char *uri;
    gint64 timestamp;
} BrowserHistoryEntry;

typedef struct {
    char *title;
    char *uri;
    gint64 timestamp;
} BrowserBookmarkEntry;

typedef struct {
    char *origin;
    char *notifications;
    char *location;
    char *cookies;
    char *camera;
    char *microphone;
    char *javascript;
    char *popups;
} BrowserSitePermissionEntry;

typedef struct BrowserState BrowserState;

BrowserState *browser_state_new(void);
void browser_state_free(BrowserState *state);

const char *browser_state_get_homepage_uri(BrowserState *state);
void browser_state_set_homepage_uri(BrowserState *state, const char *uri);

GPtrArray *browser_state_get_history(BrowserState *state);
GPtrArray *browser_state_get_bookmarks(BrowserState *state);

void browser_state_add_history(BrowserState *state, const char *title, const char *uri);
void browser_state_clear_history(BrowserState *state);

gboolean browser_state_is_bookmarked(BrowserState *state, const char *uri);
gboolean browser_state_toggle_bookmark(BrowserState *state, const char *title, const char *uri);
gboolean browser_state_remove_bookmark(BrowserState *state, const char *uri);

const char *browser_state_get_default_permission(BrowserState *state, const char *permission);
void browser_state_set_default_permission(BrowserState *state, const char *permission, const char *value);
GPtrArray *browser_state_get_site_permissions(BrowserState *state);
BrowserSitePermissionEntry *browser_state_get_site_permission(BrowserState *state, const char *origin);
const char *browser_state_get_effective_site_permission(BrowserState *state, const char *origin, const char *permission);
void browser_state_set_site_permission(BrowserState *state, const char *origin, const char *permission, const char *value);
gboolean browser_state_remove_site_permissions(BrowserState *state, const char *origin);

#endif
