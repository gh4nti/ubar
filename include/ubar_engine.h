#ifndef UBAR_ENGINE_H
#define UBAR_ENGINE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define UBAR_EXPORT __declspec(dllexport)
#define UBAR_CALL __cdecl
#else
#define UBAR_EXPORT __attribute__((visibility("default")))
#define UBAR_CALL
#endif

#define UBAR_ENGINE_ABI_V1 1u

typedef uint64_t UbarProfile;
typedef uint64_t UbarView;

typedef enum UbarResult {
    UBAR_OK = 0,
    UBAR_INVALID_ARGUMENT = 1,
    UBAR_UNSUPPORTED = 2,
    UBAR_PERMISSION_DENIED = 3,
    UBAR_ENGINE_FAILURE = 4,
    UBAR_ABI_MISMATCH = 5,
} UbarResult;

typedef enum UbarProfileKind {
    UBAR_PROFILE_NORMAL = 0,
    UBAR_PROFILE_PRIVATE = 1,
} UbarProfileKind;

typedef enum UbarEventKind {
    UBAR_NAVIGATION_STARTED = 0,
    UBAR_NAVIGATION_COMMITTED = 1,
    UBAR_NAVIGATION_FINISHED = 2,
    UBAR_TITLE_CHANGED = 3,
    UBAR_URI_CHANGED = 4,
    UBAR_RENDERER_CRASHED = 5,
    UBAR_PERMISSION_REQUESTED = 6,
    UBAR_DOWNLOAD_REQUESTED = 7,
    UBAR_MEMORY_CHANGED = 8,
    /* Internal port-to-host transport. Native shells must never receive this. */
    UBAR_EXTENSION_MESSAGE = 9,
} UbarEventKind;

typedef struct UbarBytes {
    const uint8_t *data;
    size_t len;
} UbarBytes;

typedef struct UbarOwnedBytes {
    uint8_t *data;
    size_t len;
    size_t capacity;
} UbarOwnedBytes;

typedef struct UbarProfileConfigV1 {
    uint32_t struct_size;
    UbarProfileKind kind;
    UbarBytes data_directory_utf8;
    UbarBytes cache_directory_utf8;
    uint64_t memory_target_bytes;
    uint64_t memory_ceiling_bytes;
    bool partition_third_party_storage;
    bool block_third_party_cookies;
    bool require_sandbox;
} UbarProfileConfigV1;

typedef struct UbarViewConfigV1 {
    uint32_t struct_size;
    void *native_parent;
    uint32_t width;
    uint32_t height;
    double device_scale;
    bool initially_visible;
} UbarViewConfigV1;

typedef struct UbarEventV1 {
    uint32_t struct_size;
    UbarEventKind kind;
    UbarView view;
    UbarBytes text_utf8;
    uint64_t value;
} UbarEventV1;

typedef void(UBAR_CALL *UbarEventCallbackV1)(void *user_data, const UbarEventV1 *event);

typedef struct UbarCallbacksV1 {
    uint32_t struct_size;
    void *user_data;
    UbarEventCallbackV1 event;
} UbarCallbacksV1;

typedef struct UbarEngineApiV1 {
    uint32_t abi_version;
    uint32_t struct_size;
    const char *engine_name;
    UbarResult(UBAR_CALL *create_profile)(const UbarProfileConfigV1 *, UbarProfile *);
    UbarResult(UBAR_CALL *destroy_profile)(UbarProfile);
    UbarResult(UBAR_CALL *create_view)(UbarProfile, const UbarViewConfigV1 *, const UbarCallbacksV1 *, UbarView *);
    UbarResult(UBAR_CALL *destroy_view)(UbarView);
    UbarResult(UBAR_CALL *navigate)(UbarView, UbarBytes);
    UbarResult(UBAR_CALL *set_visible)(UbarView, bool);
    UbarResult(UBAR_CALL *set_zoom)(UbarView, double);
    UbarResult(UBAR_CALL *suspend)(UbarView);
    UbarResult(UBAR_CALL *resume)(UbarView);
    UbarResult(UBAR_CALL *set_request_policy_json)(UbarProfile, UbarBytes);
    UbarResult(UBAR_CALL *register_cdm)(UbarProfile, UbarBytes, UbarBytes, UbarBytes);
    void(UBAR_CALL *free_bytes)(UbarOwnedBytes);
    UbarResult(UBAR_CALL *go_back)(UbarView);
    UbarResult(UBAR_CALL *go_forward)(UbarView);
    UbarResult(UBAR_CALL *reload)(UbarView);
    UbarResult(UBAR_CALL *stop)(UbarView);
    UbarResult(UBAR_CALL *extension_control_json)(UbarProfile, UbarBytes, UbarOwnedBytes *);
    UbarResult(UBAR_CALL *browser_control_json)(uint64_t profile,
        UbarBytes request_json_utf8, UbarOwnedBytes *response_json_utf8_out);
} UbarEngineApiV1;

typedef UbarResult(UBAR_CALL *UbarGetEngineApi)(uint32_t, const UbarEngineApiV1 **);

UBAR_EXPORT UbarResult UBAR_CALL ubar_get_engine_api(uint32_t requested_abi, const UbarEngineApiV1 **api_out);

#endif
