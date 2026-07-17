#ifndef UBAR_SHELL_ENGINE_BRIDGE_H
#define UBAR_SHELL_ENGINE_BRIDGE_H

#include <stdbool.h>
#include <stdint.h>

typedef struct UbarShellEngine UbarShellEngine;
typedef void (*UbarShellEventCallback)(void *context, uint32_t kind, uint64_t view,
                                      const char *text, uint64_t value);

UbarShellEngine *ubar_shell_engine_open(bool private_mode, uint64_t physical_memory);
const char *ubar_shell_engine_status(UbarShellEngine *engine);
void ubar_shell_engine_set_event_callback(UbarShellEngine *engine,
    UbarShellEventCallback callback, void *context);
uint64_t ubar_shell_engine_create_view(UbarShellEngine *engine, void *native_parent);
void ubar_shell_engine_destroy_view(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_set_visible(UbarShellEngine *engine, uint64_t view, bool visible);
void ubar_shell_engine_navigate(UbarShellEngine *engine, uint64_t view, const char *input);
void ubar_shell_engine_go_back(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_go_forward(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_reload(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_stop(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_set_zoom(UbarShellEngine *engine, uint64_t view, double zoom);
char *ubar_shell_engine_extension_control(UbarShellEngine *engine, const char *request_json);
char *ubar_shell_engine_browser_control(UbarShellEngine *engine, const char *request_json);
void ubar_shell_engine_free_string(char *value);
void ubar_shell_engine_close(UbarShellEngine *engine);

#endif
