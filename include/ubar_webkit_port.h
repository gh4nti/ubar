#ifndef UBAR_WEBKIT_PORT_H
#define UBAR_WEBKIT_PORT_H

#include "ubar_engine.h"

#define UBAR_WEBKIT_PORT_ABI_V1 1u

#ifdef __cplusplus
extern "C" {
#endif

typedef struct UbarWebKitPortApiV1 {
    uint32_t abi_version;
    uint32_t struct_size;
    UbarResult (*create_profile)(uint64_t id, const UbarProfileConfigV1* config);
    UbarResult (*destroy_profile)(uint64_t id);
    UbarResult (*create_view)(uint64_t id, uint64_t profile,
                              const UbarViewConfigV1* config,
                              const UbarCallbacksV1* callbacks);
    UbarResult (*destroy_view)(uint64_t id);
    UbarResult (*navigate)(uint64_t id, UbarBytes uri_utf8);
    UbarResult (*set_visible)(uint64_t id, bool visible);
    UbarResult (*set_zoom)(uint64_t id, double zoom);
    UbarResult (*suspend)(uint64_t id);
    UbarResult (*resume)(uint64_t id);
    UbarResult (*go_back)(uint64_t id);
    UbarResult (*go_forward)(uint64_t id);
    UbarResult (*reload)(uint64_t id);
    UbarResult (*stop)(uint64_t id);
    UbarResult (*set_request_policy_json)(uint64_t profile, UbarBytes policy_json);
    UbarResult (*evaluate_extension_script)(UbarView view,
                                            UbarBytes world_utf8,
                                            UbarBytes script_utf8);
    UbarResult (*create_headless_view)(uint64_t id, uint64_t profile,
                                      const UbarCallbacksV1* callbacks);
} UbarWebKitPortApiV1;

UBAR_EXPORT UbarResult ubar_webkit_port_get_api(
    uint32_t requested_abi,
    const UbarWebKitPortApiV1** api_out);

#ifdef __cplusplus
}
#endif

#endif
