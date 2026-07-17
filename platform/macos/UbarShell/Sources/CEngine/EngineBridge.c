#include "EngineBridge.h"
#include "../../../../../include/ubar_engine.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct UbarShellEngine {
    void *library;
    const UbarEngineApiV1 *api;
    UbarProfile profile;
    UbarShellEventCallback event_callback;
    void *event_context;
    char status[96];
};

static void on_engine_event(void *user_data, const UbarEventV1 *event) {
    UbarShellEngine *engine = user_data;
    if (!engine || !engine->event_callback || !event) return;
    char *text = calloc(event->text_utf8.len + 1, 1);
    if (!text) return;
    if (event->text_utf8.data && event->text_utf8.len)
        memcpy(text, event->text_utf8.data, event->text_utf8.len);
    engine->event_callback(engine->event_context, event->kind, event->view, text, event->value);
    free(text);
}

UbarShellEngine *ubar_shell_engine_open(bool private_mode, uint64_t physical_memory) {
    UbarShellEngine *engine = calloc(1, sizeof(*engine));
    if (!engine) return NULL;
    engine->library = dlopen("libubar_engine.dylib", RTLD_NOW | RTLD_LOCAL);
    if (!engine->library) {
        snprintf(engine->status, sizeof(engine->status), "engine ABI unavailable");
        return engine;
    }
    UbarGetEngineApi get_api = (UbarGetEngineApi)dlsym(engine->library, "ubar_get_engine_api");
    if (!get_api || get_api(UBAR_ENGINE_ABI_V1, &engine->api) != UBAR_OK) {
        snprintf(engine->status, sizeof(engine->status), "engine ABI mismatch");
        return engine;
    }
    UbarProfileConfigV1 profile = {
        .struct_size = sizeof(profile),
        .kind = private_mode ? UBAR_PROFILE_PRIVATE : UBAR_PROFILE_NORMAL,
        .memory_target_bytes = physical_memory / 4,
        .memory_ceiling_bytes = physical_memory * 3 / 4,
        .partition_third_party_storage = true,
        .block_third_party_cookies = true,
        .require_sandbox = true,
    };
    if (engine->api->create_profile(&profile, &engine->profile) != UBAR_OK) {
        snprintf(engine->status, sizeof(engine->status), "profile creation failed");
        return engine;
    }
    snprintf(engine->status, sizeof(engine->status), "%s", engine->api->engine_name);
    return engine;
}

const char *ubar_shell_engine_status(UbarShellEngine *engine) {
    return engine ? engine->status : "engine unavailable";
}

void ubar_shell_engine_set_event_callback(UbarShellEngine *engine,
    UbarShellEventCallback callback, void *context) {
    if (!engine) return;
    engine->event_callback = callback;
    engine->event_context = context;
}

uint64_t ubar_shell_engine_create_view(UbarShellEngine *engine, void *native_parent) {
    if (!engine || !engine->api || !engine->profile || !native_parent) return 0;
    UbarView view = 0;
    UbarViewConfigV1 config = {
        .struct_size = sizeof(config), .native_parent = native_parent,
        .width = 1100, .height = 720, .device_scale = 1.0, .initially_visible = true,
    };
    UbarCallbacksV1 callbacks = {
        .struct_size = sizeof(callbacks), .user_data = engine, .event = on_engine_event,
    };
    return engine->api->create_view(engine->profile, &config, &callbacks, &view) == UBAR_OK ? view : 0;
}

void ubar_shell_engine_destroy_view(UbarShellEngine *engine, uint64_t view) {
    if (engine && engine->api && view) engine->api->destroy_view(view);
}

void ubar_shell_engine_set_visible(UbarShellEngine *engine, uint64_t view, bool visible) {
    if (engine && engine->api && view) engine->api->set_visible(view, visible);
}

void ubar_shell_engine_navigate(UbarShellEngine *engine, uint64_t view, const char *input) {
    if (!engine || !engine->api || !view || !input || !*input) return;
    char uri[4096];
    if (strstr(input, "://")) snprintf(uri, sizeof(uri), "%s", input);
    else snprintf(uri, sizeof(uri), "https://%s", input);
    UbarBytes bytes = {(const uint8_t *)uri, strlen(uri)};
    engine->api->navigate(view, bytes);
}

void ubar_shell_engine_go_back(UbarShellEngine *engine, uint64_t view) {
    if (engine && engine->api && view) engine->api->go_back(view);
}

void ubar_shell_engine_go_forward(UbarShellEngine *engine, uint64_t view) {
    if (engine && engine->api && view) engine->api->go_forward(view);
}

void ubar_shell_engine_reload(UbarShellEngine *engine, uint64_t view) {
    if (engine && engine->api && view) engine->api->reload(view);
}

void ubar_shell_engine_stop(UbarShellEngine *engine, uint64_t view) {
    if (engine && engine->api && view) engine->api->stop(view);
}

void ubar_shell_engine_set_zoom(UbarShellEngine *engine, uint64_t view, double zoom) {
    if (engine && engine->api && view) engine->api->set_zoom(view, zoom);
}

char *ubar_shell_engine_extension_control(UbarShellEngine *engine, const char *request_json) {
    if (!engine || !engine->api || !engine->profile || !request_json ||
        !engine->api->extension_control_json) return NULL;
    UbarBytes request = {(const uint8_t *)request_json, strlen(request_json)};
    UbarOwnedBytes response = {0};
    if (engine->api->extension_control_json(engine->profile, request, &response) != UBAR_OK ||
        !response.data) return NULL;
    char *copy = malloc(response.len + 1);
    if (copy) {
        memcpy(copy, response.data, response.len);
        copy[response.len] = '\0';
    }
    engine->api->free_bytes(response);
    return copy;
}

char *ubar_shell_engine_browser_control(UbarShellEngine *engine, const char *request_json) {
    if (!engine || !engine->api || !engine->profile || !request_json ||
        !engine->api->browser_control_json) return NULL;
    UbarBytes request = {(const uint8_t *)request_json, strlen(request_json)};
    UbarOwnedBytes response = {0};
    if (engine->api->browser_control_json(engine->profile, request, &response) != UBAR_OK ||
        !response.data) return NULL;
    char *copy = malloc(response.len + 1);
    if (copy) {
        memcpy(copy, response.data, response.len);
        copy[response.len] = '\0';
    }
    engine->api->free_bytes(response);
    return copy;
}

void ubar_shell_engine_free_string(char *value) { free(value); }

void ubar_shell_engine_close(UbarShellEngine *engine) {
    if (!engine) return;
    if (engine->api && engine->profile) engine->api->destroy_profile(engine->profile);
    if (engine->library) dlclose(engine->library);
    free(engine);
}
