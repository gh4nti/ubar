#include "ubar_webkit_port.h"

#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

#include <cstring>
#include <filesystem>
#include <memory>
#include <string>
#include <unordered_map>

namespace {

struct ProfileState {
    WebKitWebsiteDataManager* data_manager {};
    WebKitWebContext* context {};
    WebKitUserContentFilterStore* filter_store {};
    WebKitUserContentFilter* filter {};
    std::string filter_path;
    bool private_profile {};
};

struct ViewState {
    uint64_t profile {};
    WebKitWebView* web_view {};
    UbarCallbacksV1 callbacks {};
    std::string default_user_agent;
};

std::unordered_map<uint64_t, ProfileState> profiles;
std::unordered_map<uint64_t, ViewState> views;

std::string copy_bytes(UbarBytes value)
{
    if (!value.data || !value.len)
        return {};
    return { reinterpret_cast<const char*>(value.data), value.len };
}

void emit(uint64_t id, UbarEventKind kind, const char* text = nullptr, uint64_t value = 0)
{
    auto found = views.find(id);
    if (found == views.end() || !found->second.callbacks.event)
        return;
    const auto length = text ? std::strlen(text) : 0;
    UbarEventV1 event {
        sizeof(UbarEventV1), kind, id,
        { reinterpret_cast<const uint8_t*>(text), length }, value
    };
    found->second.callbacks.event(found->second.callbacks.user_data, &event);
}

uint64_t view_id(WebKitWebView* view)
{
    auto* value = static_cast<uint64_t*>(g_object_get_data(G_OBJECT(view), "ubar-view-id"));
    return value ? *value : 0;
}

void load_changed(WebKitWebView* view, WebKitLoadEvent event, gpointer)
{
    const auto id = view_id(view);
    switch (event) {
    case WEBKIT_LOAD_STARTED:
        emit(id, UBAR_NAVIGATION_STARTED, webkit_web_view_get_uri(view));
        break;
    case WEBKIT_LOAD_COMMITTED:
        emit(id, UBAR_NAVIGATION_COMMITTED, webkit_web_view_get_uri(view));
        break;
    case WEBKIT_LOAD_FINISHED:
        emit(id, UBAR_NAVIGATION_FINISHED, webkit_web_view_get_uri(view));
        break;
    default:
        break;
    }
}

void title_changed(GObject* object, GParamSpec*, gpointer)
{
    auto* view = WEBKIT_WEB_VIEW(object);
    emit(view_id(view), UBAR_TITLE_CHANGED, webkit_web_view_get_title(view));
}

void uri_changed(GObject* object, GParamSpec*, gpointer)
{
    auto* view = WEBKIT_WEB_VIEW(object);
    emit(view_id(view), UBAR_URI_CHANGED, webkit_web_view_get_uri(view));
}

gboolean web_process_terminated(WebKitWebView* view, WebKitWebProcessTerminationReason reason, gpointer)
{
    emit(view_id(view), UBAR_RENDERER_CRASHED, nullptr, static_cast<uint64_t>(reason));
    return FALSE;
}

gboolean permission_requested(WebKitWebView* view, WebKitPermissionRequest*, gpointer)
{
    emit(view_id(view), UBAR_PERMISSION_REQUESTED, webkit_web_view_get_uri(view));
    return FALSE;
}

const char* masked_user_agent(const std::string& uri, const std::string& fallback)
{
    static const char* firefox =
        "Mozilla/5.0 (X11; Linux x86_64; rv:152.0) Gecko/20100101 Firefox/152.0";
    static const char* chrome =
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/152.0.0.0 Safari/537.36";
    static const char* edge =
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/152.0.0.0 Safari/537.36 Edg/152.0.0.0";
    static const char* opera =
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
        "Chrome/152.0.0.0 Safari/537.36 OPR/126.0.0.0";
    if (uri.find("addons.mozilla.org") != std::string::npos)
        return firefox;
    if (uri.find("chromewebstore.google.com") != std::string::npos)
        return chrome;
    if (uri.find("microsoftedge.microsoft.com/addons") != std::string::npos)
        return edge;
    if (uri.find("addons.opera.com") != std::string::npos)
        return opera;
    return fallback.c_str();
}

UbarResult create_profile(uint64_t id, const UbarProfileConfigV1* config)
{
    if (!id || !config || config->struct_size < sizeof(UbarProfileConfigV1)
        || profiles.contains(id) || !config->require_sandbox)
        return UBAR_INVALID_ARGUMENT;

    WebKitWebsiteDataManager* manager = nullptr;
    if (config->kind == UBAR_PROFILE_PRIVATE) {
        manager = webkit_website_data_manager_new_ephemeral();
    } else {
        const auto data = copy_bytes(config->data_directory_utf8);
        const auto cache = copy_bytes(config->cache_directory_utf8);
        manager = webkit_website_data_manager_new(
            "base-data-directory", data.empty() ? nullptr : data.c_str(),
            "base-cache-directory", cache.empty() ? nullptr : cache.c_str(), nullptr);
    }
    if (!manager)
        return UBAR_ENGINE_FAILURE;

    auto* context = webkit_web_context_new_with_website_data_manager(manager);
    if (!context) {
        g_object_unref(manager);
        return UBAR_ENGINE_FAILURE;
    }
    webkit_web_context_set_sandbox_enabled(context, TRUE);
    webkit_cookie_manager_set_accept_policy(
        webkit_web_context_get_cookie_manager(context),
        config->block_third_party_cookies
            ? WEBKIT_COOKIE_POLICY_ACCEPT_NO_THIRD_PARTY
            : WEBKIT_COOKIE_POLICY_ACCEPT_ALWAYS);
    const auto cache = copy_bytes(config->cache_directory_utf8);
    std::string filter_path;
    if (config->kind == UBAR_PROFILE_PRIVATE) {
        GError* error = nullptr;
        gchar* path = g_dir_make_tmp("ubar-private-filters-XXXXXX", &error);
        if (!path) {
            if (error) g_error_free(error);
            g_object_unref(context);
            g_object_unref(manager);
            return UBAR_ENGINE_FAILURE;
        }
        filter_path = path;
        g_free(path);
    } else {
        filter_path = cache.empty()
            ? std::string(g_get_user_cache_dir()) + "/ubar/content-filters"
            : cache + "/content-filters";
    }
    auto* filter_store = webkit_user_content_filter_store_new(filter_path.c_str());
    if (!filter_store) {
        if (config->kind == UBAR_PROFILE_PRIVATE) std::filesystem::remove_all(filter_path);
        g_object_unref(context);
        g_object_unref(manager);
        return UBAR_ENGINE_FAILURE;
    }
    profiles.emplace(id, ProfileState {
        manager, context, filter_store, nullptr, filter_path,
        config->kind == UBAR_PROFILE_PRIVATE
    });
    return UBAR_OK;
}

UbarResult destroy_profile(uint64_t id)
{
    auto found = profiles.find(id);
    if (found == profiles.end())
        return UBAR_INVALID_ARGUMENT;
    for (const auto& [_, view] : views) {
        if (webkit_web_view_get_context(view.web_view) == found->second.context)
            return UBAR_PERMISSION_DENIED;
    }
    g_object_unref(found->second.context);
    g_object_unref(found->second.data_manager);
    if (found->second.filter) webkit_user_content_filter_unref(found->second.filter);
    g_object_unref(found->second.filter_store);
    if (found->second.private_profile)
        std::filesystem::remove_all(found->second.filter_path);
    profiles.erase(found);
    return UBAR_OK;
}

UbarResult create_view(uint64_t id, uint64_t profile, const UbarViewConfigV1* config,
                       const UbarCallbacksV1* callbacks)
{
    auto owner = profiles.find(profile);
    if (!id || owner == profiles.end() || !config || !config->native_parent || views.contains(id))
        return UBAR_INVALID_ARGUMENT;
    auto* widget = webkit_web_view_new_with_context(owner->second.context);
    auto* web_view = WEBKIT_WEB_VIEW(widget);
    auto* settings = webkit_web_view_get_settings(web_view);
    webkit_settings_set_hardware_acceleration_policy(
        settings, WEBKIT_HARDWARE_ACCELERATION_POLICY_ALWAYS);
    webkit_settings_set_enable_webaudio(settings, TRUE);
    webkit_settings_set_enable_media_stream(settings, TRUE);
    webkit_settings_set_enable_mediasource(settings, TRUE);
    webkit_settings_set_enable_webrtc(settings, TRUE);
    const char* current_ua = webkit_settings_get_user_agent(settings);
    ViewState state { profile, web_view, {}, current_ua ? current_ua : "" };
    if (callbacks)
        state.callbacks = *callbacks;
    views.emplace(id, std::move(state));
    if (owner->second.filter)
        webkit_user_content_manager_add_filter(
            webkit_web_view_get_user_content_manager(web_view), owner->second.filter);
    auto* stored_id = g_new(uint64_t, 1);
    *stored_id = id;
    g_object_set_data_full(G_OBJECT(web_view), "ubar-view-id", stored_id, g_free);
    g_signal_connect(web_view, "load-changed", G_CALLBACK(load_changed), nullptr);
    g_signal_connect(web_view, "notify::title", G_CALLBACK(title_changed), nullptr);
    g_signal_connect(web_view, "notify::uri", G_CALLBACK(uri_changed), nullptr);
    g_signal_connect(web_view, "web-process-terminated", G_CALLBACK(web_process_terminated), nullptr);
    g_signal_connect(web_view, "permission-request", G_CALLBACK(permission_requested), nullptr);
    gtk_container_add(GTK_CONTAINER(config->native_parent), widget);
    gtk_widget_set_size_request(widget, config->width, config->height);
    if (config->initially_visible)
        gtk_widget_show(widget);
    return UBAR_OK;
}

UbarResult destroy_view(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    gtk_widget_destroy(GTK_WIDGET(found->second.web_view));
    views.erase(found);
    return UBAR_OK;
}

UbarResult navigate(uint64_t id, UbarBytes value)
{
    auto found = views.find(id);
    const auto uri = copy_bytes(value);
    if (found == views.end() || uri.empty())
        return UBAR_INVALID_ARGUMENT;
    auto* settings = webkit_web_view_get_settings(found->second.web_view);
    webkit_settings_set_user_agent(settings, masked_user_agent(uri, found->second.default_user_agent));
    webkit_web_view_load_uri(found->second.web_view, uri.c_str());
    return UBAR_OK;
}

UbarResult set_visible(uint64_t id, bool visible)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    visible ? gtk_widget_show(GTK_WIDGET(found->second.web_view))
            : gtk_widget_hide(GTK_WIDGET(found->second.web_view));
    return UBAR_OK;
}

UbarResult set_zoom(uint64_t id, double zoom)
{
    auto found = views.find(id);
    if (found == views.end() || zoom < .5 || zoom > 5.)
        return UBAR_INVALID_ARGUMENT;
    webkit_web_view_set_zoom_level(found->second.web_view, zoom);
    return UBAR_OK;
}

UbarResult suspend(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    webkit_web_view_set_is_muted(found->second.web_view, TRUE);
    gtk_widget_hide(GTK_WIDGET(found->second.web_view));
    return UBAR_OK;
}

UbarResult resume(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    webkit_web_view_set_is_muted(found->second.web_view, FALSE);
    gtk_widget_show(GTK_WIDGET(found->second.web_view));
    return UBAR_OK;
}

UbarResult go_back(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    if (!webkit_web_view_can_go_back(found->second.web_view)) return UBAR_PERMISSION_DENIED;
    webkit_web_view_go_back(found->second.web_view);
    return UBAR_OK;
}

UbarResult go_forward(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    if (!webkit_web_view_can_go_forward(found->second.web_view)) return UBAR_PERMISSION_DENIED;
    webkit_web_view_go_forward(found->second.web_view);
    return UBAR_OK;
}

UbarResult reload(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    webkit_web_view_reload(found->second.web_view);
    return UBAR_OK;
}

UbarResult stop(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    webkit_web_view_stop_loading(found->second.web_view);
    return UBAR_OK;
}

void filter_saved(GObject* source, GAsyncResult* result, gpointer user_data)
{
    std::unique_ptr<uint64_t> profile(static_cast<uint64_t*>(user_data));
    auto found = profiles.find(*profile);
    if (found == profiles.end()) return;
    GError* error = nullptr;
    auto* filter = webkit_user_content_filter_store_save_finish(
        WEBKIT_USER_CONTENT_FILTER_STORE(source), result, &error);
    if (!filter) {
        if (error) g_error_free(error);
        return;
    }
    if (found->second.filter) webkit_user_content_filter_unref(found->second.filter);
    found->second.filter = filter;
    for (auto& [_, view] : views) {
        if (view.profile != *profile) continue;
        auto* manager = webkit_web_view_get_user_content_manager(view.web_view);
        webkit_user_content_manager_remove_all_filters(manager);
        webkit_user_content_manager_add_filter(manager, filter);
    }
}

UbarResult set_request_policy_json(uint64_t profile, UbarBytes value)
{
    auto found = profiles.find(profile);
    if (found == profiles.end() || !value.data || !value.len) return UBAR_INVALID_ARGUMENT;
    auto* source = g_bytes_new(value.data, value.len);
    const auto identifier = "ubar-profile-" + std::to_string(profile);
    webkit_user_content_filter_store_save(
        found->second.filter_store, identifier.c_str(), source, nullptr,
        filter_saved, new uint64_t(profile));
    g_bytes_unref(source);
    return UBAR_OK;
}

const UbarWebKitPortApiV1 api {
    UBAR_WEBKIT_PORT_ABI_V1, sizeof(UbarWebKitPortApiV1), create_profile, destroy_profile,
    create_view, destroy_view, navigate, set_visible, set_zoom, suspend, resume,
    go_back, go_forward, reload, stop, set_request_policy_json
};

} // namespace

extern "C" UBAR_EXPORT UbarResult ubar_webkit_port_get_api(
    uint32_t requested_abi, const UbarWebKitPortApiV1** api_out)
{
    if (!api_out)
        return UBAR_INVALID_ARGUMENT;
    if (requested_abi != UBAR_WEBKIT_PORT_ABI_V1)
        return UBAR_ABI_MISMATCH;
    *api_out = &api;
    return UBAR_OK;
}
