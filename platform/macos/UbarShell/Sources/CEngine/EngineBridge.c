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
    char status[96];
};

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

uint64_t ubar_shell_engine_create_view(UbarShellEngine *engine, void *native_parent) {
    if (!engine || !engine->api || !engine->profile || !native_parent) return 0;
    UbarView view = 0;
    UbarViewConfigV1 config = {
        .struct_size = sizeof(config), .native_parent = native_parent,
        .width = 1100, .height = 720, .device_scale = 1.0, .initially_visible = true,
    };
    UbarCallbacksV1 callbacks = {.struct_size = sizeof(callbacks)};
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

void ubar_shell_engine_close(UbarShellEngine *engine) {
    if (!engine) return;
    if (engine->api && engine->profile) engine->api->destroy_profile(engine->profile);
    if (engine->library) dlclose(engine->library);
    free(engine);
}
