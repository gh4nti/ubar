#ifndef UBAR_SHELL_ENGINE_BRIDGE_H
#define UBAR_SHELL_ENGINE_BRIDGE_H

#include <stdbool.h>
#include <stdint.h>

typedef struct UbarShellEngine UbarShellEngine;

UbarShellEngine *ubar_shell_engine_open(bool private_mode, uint64_t physical_memory);
const char *ubar_shell_engine_status(UbarShellEngine *engine);
uint64_t ubar_shell_engine_create_view(UbarShellEngine *engine, void *native_parent);
void ubar_shell_engine_destroy_view(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_set_visible(UbarShellEngine *engine, uint64_t view, bool visible);
void ubar_shell_engine_navigate(UbarShellEngine *engine, uint64_t view, const char *input);
void ubar_shell_engine_go_back(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_go_forward(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_reload(UbarShellEngine *engine, uint64_t view);
void ubar_shell_engine_close(UbarShellEngine *engine);

#endif
