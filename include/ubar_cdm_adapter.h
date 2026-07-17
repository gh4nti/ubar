#ifndef UBAR_CDM_ADAPTER_H
#define UBAR_CDM_ADAPTER_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define UBAR_CDM_EXPORT __declspec(dllexport)
#else
#define UBAR_CDM_EXPORT __attribute__((visibility("default")))
#endif

#define UBAR_CDM_ADAPTER_ABI_V1 1u

typedef enum UbarCdmAdapterResult {
    UBAR_CDM_OK = 0,
    UBAR_CDM_INVALID_ARGUMENT = 1,
    UBAR_CDM_UNSUPPORTED = 2,
    UBAR_CDM_COMPONENT_REJECTED = 3,
    UBAR_CDM_FAILURE = 4,
    UBAR_CDM_ABI_MISMATCH = 5,
} UbarCdmAdapterResult;

typedef struct UbarCdmBytes { const uint8_t* data; size_t len; } UbarCdmBytes;
typedef struct UbarCdmOwnedBytes { uint8_t* data; size_t len; size_t capacity; } UbarCdmOwnedBytes;

typedef struct UbarCdmAdapterApiV1 {
    uint32_t abi_version;
    uint32_t struct_size;
    void* user_data;
    UbarCdmAdapterResult (*initialize)(void* user_data, UbarCdmBytes component_path_utf8,
        uint32_t host_version, UbarCdmOwnedBytes* module_version_utf8_out);
    UbarCdmAdapterResult (*transact)(void* user_data, UbarCdmBytes request_json,
        UbarCdmOwnedBytes* response_json_out);
    void (*shutdown)(void* user_data);
    void (*free_bytes)(void* user_data, UbarCdmOwnedBytes value);
} UbarCdmAdapterApiV1;

#ifdef __cplusplus
extern "C" {
#endif
UBAR_CDM_EXPORT UbarCdmAdapterResult ubar_get_cdm_adapter_api(
    uint32_t requested_abi, const UbarCdmAdapterApiV1** api_out);
#ifdef __cplusplus
}
#endif

#endif
