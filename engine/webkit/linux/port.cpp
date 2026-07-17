#include "ubar_webkit_port.h"

#include <gtk/gtk.h>
#include <webkit2/webkit2.h>

#include <cstring>
#include <filesystem>
#include <memory>
#include <string>
#include <unordered_map>
#include <unordered_set>

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
    std::unordered_set<std::string> bridge_worlds;
    GtkWidget* headless_window {};
};

struct DownloadState {
    uint64_t view {};
    std::string directory;
    std::string path;
    bool extension_package {};
};

bool is_extension_package_mime(const gchar* mime_type) {
    return mime_type
        && (!g_ascii_strcasecmp(mime_type, "application/x-xpinstall")
            || !g_ascii_strcasecmp(mime_type, "application/x-chrome-extension"));
}

std::string safe_download_filename(const gchar* suggested_filename) {
    gchar* basename = g_path_get_basename(
        suggested_filename && *suggested_filename ? suggested_filename : "download");
    std::string filename = basename ? basename : "download";
    g_free(basename);

    for (char& character : filename) {
        const auto byte = static_cast<unsigned char>(character);
        if (byte < 0x20 || character == '/' || character == '\\' || character == ':'
            || character == '*' || character == '?' || character == '"'
            || character == '<' || character == '>' || character == '|') {
            character = '_';
        }
    }
    while (!filename.empty() && (filename.back() == '.' || filename.back() == ' '))
        filename.pop_back();
    if (filename.empty() || filename == "." || filename == "..")
        filename = "download";
    return filename;
}

std::string collision_safe_download_path(
    const std::string& directory,
    const std::string& filename) {
    auto build = [&directory](const std::string& leaf) {
        gchar* path = g_build_filename(directory.c_str(), leaf.c_str(), nullptr);
        std::string result = path ? path : leaf;
        g_free(path);
        return result;
    };

    std::string candidate = build(filename);
    if (!g_file_test(candidate.c_str(), G_FILE_TEST_EXISTS))
        return candidate;

    const auto dot = filename.find_last_of('.');
    const bool has_extension = dot != std::string::npos && dot != 0;
    const std::string stem = has_extension ? filename.substr(0, dot) : filename;
    const std::string extension = has_extension ? filename.substr(dot) : std::string {};
    for (unsigned suffix = 1; suffix < 10000; ++suffix) {
        candidate = build(stem + " (" + std::to_string(suffix) + ")" + extension);
        if (!g_file_test(candidate.c_str(), G_FILE_TEST_EXISTS))
            return candidate;
    }
    return build(stem + "-" + std::to_string(g_get_real_time()) + extension);
}

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

void extension_message(WebKitUserContentManager*, JSCValue* value, gpointer user_data)
{
    if (!value || !jsc_value_is_string(value)) return;
    gchar* message = jsc_value_to_string(value);
    if (message) emit(*static_cast<uint64_t*>(user_data), UBAR_EXTENSION_MESSAGE, message);
    g_free(message);
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
        if (auto found = views.find(id); found != views.end()) {
            auto* manager = webkit_web_view_get_user_content_manager(view);
            for (const auto& world : found->second.bridge_worlds)
                webkit_user_content_manager_unregister_script_message_handler_in_world(
                    manager, "ubarExtensionBridge", world.c_str());
            found->second.bridge_worlds.clear();
        }
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

bool is_extension_package_uri(const char* uri)
{
    if (!uri) return false;
    const std::string value(uri);
    for (const char* suffix : { ".xpi", ".crx" }) {
        const auto extension = value.rfind(suffix);
        if (extension != std::string::npos && (extension + 4 == value.size()
            || value[extension + 4] == '?' || value[extension + 4] == '#')) return true;
    }
    return false;
}

void cleanup_download(DownloadState* state)
{
    if (!state) return;
    if (!state->path.empty()) std::filesystem::remove(state->path);
    if (!state->directory.empty()) std::filesystem::remove(state->directory);
}

gboolean decide_download_destination(WebKitDownload* download, const gchar* suggested_filename, gpointer user_data)
{
    auto* state = static_cast<DownloadState*>(user_data);
    if (auto* response = webkit_download_get_response(download)) {
        state->extension_package = state->extension_package
            || is_extension_package_mime(webkit_uri_response_get_mime_type(response))
            || is_extension_package_uri(webkit_uri_response_get_uri(response));
    }
    state->extension_package = state->extension_package
        || (suggested_filename && is_extension_package_uri(suggested_filename));

    if (!state->extension_package) {
        const gchar* downloads = g_get_user_special_dir(G_USER_DIRECTORY_DOWNLOAD);
        const gchar* home = g_get_home_dir();
        std::string directory = downloads && *downloads
            ? downloads
            : std::string(home && *home ? home : ".") + G_DIR_SEPARATOR_S + "Downloads";
        std::error_code directory_error;
        std::filesystem::create_directories(directory, directory_error);
        if (directory_error)
            return FALSE;

        state->path = collision_safe_download_path(
            directory, safe_download_filename(suggested_filename));
        auto* file = g_file_new_for_path(state->path.c_str());
        gchar* destination = g_file_get_uri(file);
        webkit_download_set_allow_overwrite(download, FALSE);
        webkit_download_set_destination(download, destination);
        g_free(destination);
        g_object_unref(file);
        return TRUE;
    }

    GError* error = nullptr;
    gchar* directory = g_dir_make_tmp("ubar-xpi-XXXXXX", &error);
    if (!directory) {
        if (error) g_error_free(error);
        return FALSE;
    }
    state->directory = directory;
    state->path = state->directory + "/package.xpi";
    auto* file = g_file_new_for_path(state->path.c_str());
    gchar* destination = g_file_get_uri(file);
    webkit_download_set_allow_overwrite(download, FALSE);
    webkit_download_set_destination(download, destination);
    g_free(destination);
    g_object_unref(file);
    g_free(directory);
    return TRUE;
}

void download_finished(WebKitDownload*, gpointer user_data)
{
    std::unique_ptr<DownloadState> state(static_cast<DownloadState*>(user_data));
    if (!state->extension_package)
        return;
    emit(state->view, UBAR_DOWNLOAD_REQUESTED, state->path.c_str());
    cleanup_download(state.get());
}

void download_failed(WebKitDownload*, GError*, gpointer user_data)
{
    std::unique_ptr<DownloadState> state(static_cast<DownloadState*>(user_data));
    cleanup_download(state.get());
}

void download_started(WebKitWebContext*, WebKitDownload* download, gpointer)
{
    auto* request = webkit_download_get_request(download);
    const bool extension_package = request
        && is_extension_package_uri(webkit_uri_request_get_uri(request));
    auto* web_view = webkit_download_get_web_view(download);
    auto* state = new DownloadState {
        web_view ? view_id(web_view) : 0, {}, {}, extension_package
    };
    if (!state->view) { delete state; return; }
    g_signal_connect(download, "decide-destination", G_CALLBACK(decide_download_destination), state);
    g_signal_connect(download, "failed", G_CALLBACK(download_failed), state);
    g_signal_connect_data(download, "finished", G_CALLBACK(download_finished), state,
        [](gpointer data, GClosure*) { delete static_cast<DownloadState*>(data); },
        G_CONNECT_DEFAULT);
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
    g_signal_connect(context, "download-started", G_CALLBACK(download_started), nullptr);
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
    ViewState state { profile, web_view, {}, current_ua ? current_ua : "", {} };
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
    auto* bridge_id = new uint64_t(id);
    g_signal_connect_data(webkit_web_view_get_user_content_manager(web_view),
        "script-message-received::ubarExtensionBridge", G_CALLBACK(extension_message), bridge_id,
        [](gpointer data, GClosure*) { delete static_cast<uint64_t*>(data); }, G_CONNECT_DEFAULT);
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
    gtk_widget_destroy(found->second.headless_window
        ? found->second.headless_window : GTK_WIDGET(found->second.web_view));
    views.erase(found);
    return UBAR_OK;
}

UbarResult create_headless_view(uint64_t id, uint64_t profile, const UbarCallbacksV1* callbacks)
{
    auto* window = gtk_offscreen_window_new();
    UbarViewConfigV1 config { sizeof(UbarViewConfigV1), window, 1, 1, 1., true };
    const auto result = create_view(id, profile, &config, callbacks);
    if (result != UBAR_OK) gtk_widget_destroy(window);
    else views.at(id).headless_window = window;
    return result;
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
    go_back, go_forward, reload, stop, set_request_policy_json,
    [](UbarView view, UbarBytes world_utf8, UbarBytes script_utf8) -> UbarResult {
      auto found = views.find(view);
      if (found == views.end()) {
        return UBAR_INVALID_ARGUMENT;
      }
      const auto world = copy_bytes(world_utf8);
      auto script = copy_bytes(script_utf8);
      if (!world.empty()) {
        auto* manager = webkit_web_view_get_user_content_manager(found->second.web_view);
        if (found->second.bridge_worlds.insert(world).second
            && !webkit_user_content_manager_register_script_message_handler_in_world(
                manager, "ubarExtensionBridge", world.c_str()))
          return UBAR_ENGINE_FAILURE;
        const auto world_literal = g_strescape(world.c_str(), nullptr);
        std::string bridge = "if(!globalThis.__ubarExtensionBridge)Object.defineProperty(globalThis,'__ubarExtensionBridge',{value:{postMessage(m){webkit.messageHandlers.ubarExtensionBridge.postMessage(JSON.stringify({world:'";
        bridge += world_literal;
        bridge += "',message:String(m)}));}},configurable:false});\n";
        g_free(world_literal);
        script.insert(0, bridge);
      }
      webkit_web_view_evaluate_javascript(found->second.web_view,
                                          script.c_str(), script.size(),
                                          world.empty() ? nullptr : world.c_str(),
                                          nullptr, nullptr, nullptr, nullptr);
      return UBAR_OK;
    },
    create_headless_view
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
