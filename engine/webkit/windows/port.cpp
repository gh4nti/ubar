#include "ubar_webkit_port.h"

#include <windows.h>
#include <shlobj.h>
#include <WebKit/WebKit2_C.h>
#include <WebKit/WKDownloadRef.h>
#include <WebKit/WKFramePolicyListener.h>
#include <WebKit/WKNavigationActionRef.h>
#include <WebKit/WKNavigationResponseRef.h>
#include <WebKit/WKURLRequest.h>
#include <WebKit/WKURLResponse.h>

#include <algorithm>
#include <atomic>
#include <cctype>
#include <cwchar>
#include <filesystem>
#include <string>
#include <memory>
#include <unordered_map>
#include <vector>

namespace {

struct ProfileState {
    WKContextRef context {};
    WKWebsiteDataStoreRef dataStore {};
    WKPreferencesRef preferences {};
    WKUserContentControllerRef userContentController {};
    WKUserContentExtensionStoreRef contentExtensionStore {};
};

struct ViewState {
    uint64_t profile {};
    WKViewRef view {};
    UbarCallbacksV1 callbacks {};
    HWND headless_parent {};
};

struct DownloadState {
    uint64_t view {};
    std::filesystem::path directory;
    std::filesystem::path path;
    bool extension_package {};
    WKDownloadClientV0 client {};
};

std::unordered_map<uint64_t, ProfileState> profiles;
std::unordered_map<uint64_t, ViewState> views;
std::unordered_map<WKDownloadRef, std::unique_ptr<DownloadState>> downloads;
std::atomic_uint64_t next_download { 1 };

std::string copy_bytes(UbarBytes value)
{
    if (!value.data || !value.len) return {};
    return { reinterpret_cast<const char*>(value.data), value.len };
}

WKStringRef make_string(const std::string& value)
{
    return WKStringCreateWithUTF8CString(value.c_str());
}

std::string copy_string(WKStringRef value)
{
    if (!value) return {};
    std::vector<char> buffer(WKStringGetMaximumUTF8CStringSize(value));
    const auto length = WKStringGetUTF8CString(value, buffer.data(), buffer.size());
    return length ? std::string(buffer.data(), length - 1) : std::string();
}

std::string copy_url(WKURLRef value)
{
    if (!value) return {};
    auto string = WKURLCopyString(value);
    auto result = copy_string(string);
    WKRelease(string);
    return result;
}

std::string wide_to_utf8(const std::wstring& value)
{
    if (value.empty()) return {};
    const int length = WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()),
        nullptr, 0, nullptr, nullptr);
    std::string result(length, '\0');
    WideCharToMultiByte(CP_UTF8, 0, value.data(), static_cast<int>(value.size()),
        result.data(), length, nullptr, nullptr);
    return result;
}

std::wstring utf8_to_wide(const std::string& value)
{
    if (value.empty()) return {};
    const int length = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
        static_cast<int>(value.size()), nullptr, 0);
    if (!length) return {};
    std::wstring result(length, L'\0');
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
        static_cast<int>(value.size()), result.data(), length);
    return result;
}

std::wstring safe_download_filename(const std::string& suggested)
{
    auto filename = std::filesystem::path(utf8_to_wide(suggested)).filename().wstring();
    if (filename.empty()) filename = L"download";
    for (auto& character : filename) {
        if (character < 0x20 || wcschr(L"\\/:*?\"<>|", character)) character = L'_';
    }
    while (!filename.empty() && (filename.back() == L'.' || filename.back() == L' '))
        filename.pop_back();
    return filename.empty() || filename == L"." || filename == L".." ? L"download" : filename;
}

std::filesystem::path downloads_directory()
{
    PWSTR path = nullptr;
    if (FAILED(SHGetKnownFolderPath(FOLDERID_Downloads, KF_FLAG_CREATE, nullptr, &path)))
        return {};
    std::filesystem::path result(path);
    CoTaskMemFree(path);
    return result;
}

std::filesystem::path collision_safe_path(
    const std::filesystem::path& directory, const std::wstring& filename)
{
    auto candidate = directory / filename;
    std::error_code error;
    if (!std::filesystem::exists(candidate, error)) return candidate;
    const std::filesystem::path name(filename);
    const auto stem = name.stem().wstring();
    const auto extension = name.has_extension() ? name.extension().wstring() : std::wstring();
    for (unsigned suffix = 1; suffix < 10000; ++suffix) {
        candidate = directory / (stem + L" (" + std::to_wstring(suffix) + L")" + extension);
        error.clear();
        if (!std::filesystem::exists(candidate, error)) return candidate;
    }
    return directory / (stem + L"-" + std::to_wstring(GetTickCount64()) + extension);
}

uint64_t view_id(WKPageRef page)
{
    for (const auto& [id, state] : views)
        if (WKViewGetPage(state.view) == page) return id;
    return 0;
}

void emit(WKPageRef page, UbarEventKind kind, const std::string& text = {}, uint64_t value = 0)
{
    const auto id = view_id(page);
    auto found = views.find(id);
    if (!id || found == views.end() || !found->second.callbacks.event) return;
    UbarEventV1 event {
        sizeof(UbarEventV1), kind, id,
        { reinterpret_cast<const uint8_t*>(text.data()), text.size() }, value
    };
    found->second.callbacks.event(found->second.callbacks.user_data, &event);
}

std::string active_url(WKPageRef page)
{
    auto url = WKPageCopyActiveURL(page);
    auto value = copy_url(url);
    if (url) WKRelease(url);
    return value;
}

bool is_xpi(const std::string& uri, const std::string& mime = {})
{
    std::string lowered_uri = uri;
    std::string lowered_mime = mime;
    auto lower = [](unsigned char value) { return static_cast<char>(std::tolower(value)); };
    std::transform(lowered_uri.begin(), lowered_uri.end(), lowered_uri.begin(), lower);
    std::transform(lowered_mime.begin(), lowered_mime.end(), lowered_mime.begin(), lower);
    if (lowered_mime == "application/x-xpinstall"
        || lowered_mime == "application/x-chrome-extension") return true;
    for (const char* suffix : { ".xpi", ".crx" }) {
        const auto extension = lowered_uri.rfind(suffix);
        if (extension != std::string::npos && (extension + 4 == lowered_uri.size()
            || lowered_uri[extension + 4] == '?' || lowered_uri[extension + 4] == '#')) return true;
    }
    return false;
}

void navigation_action(WKPageRef, WKNavigationActionRef action,
    WKFramePolicyListenerRef listener, WKTypeRef, const void*)
{
    auto request = WKNavigationActionCopyRequest(action);
    auto url = request ? WKURLRequestCopyURL(request) : nullptr;
    const bool download = WKNavigationActionShouldPerformDownload(action) || is_xpi(copy_url(url));
    if (url) WKRelease(url);
    if (request) WKRelease(request);
    download ? WKFramePolicyListenerDownload(listener) : WKFramePolicyListenerUse(listener);
}

void navigation_response(WKPageRef, WKNavigationResponseRef navigation,
    WKFramePolicyListenerRef listener, WKTypeRef, const void*)
{
    auto response = WKNavigationResponseCopyResponse(navigation);
    auto url = response ? WKURLResponseCopyURL(response) : nullptr;
    auto mime = response ? WKURLResponseCopyMIMEType(response) : nullptr;
    const bool download = !WKNavigationResponseCanShowMIMEType(navigation)
        || is_xpi(copy_url(url), copy_string(mime));
    if (mime) WKRelease(mime);
    if (url) WKRelease(url);
    if (response) WKRelease(response);
    download ? WKFramePolicyListenerDownload(listener) : WKFramePolicyListenerUse(listener);
}

void navigation_started(WKPageRef page, WKNavigationRef, WKTypeRef, const void*)
{
    emit(page, UBAR_NAVIGATION_STARTED, active_url(page));
}

void navigation_committed(WKPageRef page, WKNavigationRef, WKTypeRef, const void*)
{
    const auto uri = active_url(page);
    emit(page, UBAR_NAVIGATION_COMMITTED, uri);
    emit(page, UBAR_URI_CHANGED, uri);
}

void navigation_finished(WKPageRef page, WKNavigationRef, WKTypeRef, const void*)
{
    emit(page, UBAR_NAVIGATION_FINISHED, active_url(page));
    auto title = WKPageCopyTitle(page);
    emit(page, UBAR_TITLE_CHANGED, copy_string(title));
    if (title) WKRelease(title);
}

void web_process_crashed(WKPageRef page, const void*)
{
    emit(page, UBAR_RENDERER_CRASHED);
}

void web_process_terminated(WKPageRef page, WKProcessTerminationReason reason, const void*)
{
    emit(page, UBAR_RENDERER_CRASHED, {}, static_cast<uint64_t>(reason));
}

WKStringRef download_destination(WKDownloadRef, WKURLResponseRef response,
    WKStringRef suggested_filename, const void* client_info)
{
    auto* state = static_cast<DownloadState*>(const_cast<void*>(client_info));
    auto url = response ? WKURLResponseCopyURL(response) : nullptr;
    auto mime = response ? WKURLResponseCopyMIMEType(response) : nullptr;
    const auto suggested = copy_string(suggested_filename);
    state->extension_package = is_xpi(copy_url(url), copy_string(mime)) || is_xpi(suggested);
    if (mime) WKRelease(mime);
    if (url) WKRelease(url);

    std::error_code error;
    if (!state->extension_package) {
        state->directory = downloads_directory();
        if (state->directory.empty()) return nullptr;
        std::filesystem::create_directories(state->directory, error);
        if (error) return nullptr;
        state->path = collision_safe_path(state->directory, safe_download_filename(suggested));
        return make_string(wide_to_utf8(state->path.wstring()));
    }

    auto base = std::filesystem::temp_directory_path(error);
    if (error) return nullptr;
    for (unsigned attempt = 0; attempt < 32; ++attempt) {
        state->directory = base / ("ubar-xpi-" + std::to_string(GetCurrentProcessId()) + "-"
            + std::to_string(next_download.fetch_add(1)));
        if (std::filesystem::create_directory(state->directory, error)) break;
        error.clear();
    }
    if (!std::filesystem::is_directory(state->directory, error)) return nullptr;
    state->path = state->directory / "package.xpi";
    return make_string(wide_to_utf8(state->path.wstring()));
}

void download_finished(WKDownloadRef download, const void* client_info)
{
    auto* state = static_cast<const DownloadState*>(client_info);
    if (!state->extension_package) {
        downloads.erase(download);
        return;
    }
    auto found = views.find(state->view);
    if (found != views.end() && found->second.callbacks.event) {
        const auto path = wide_to_utf8(state->path.wstring());
        UbarEventV1 event { sizeof(UbarEventV1), UBAR_DOWNLOAD_REQUESTED, state->view,
            { reinterpret_cast<const uint8_t*>(path.data()), path.size() }, 0 };
        found->second.callbacks.event(found->second.callbacks.user_data, &event);
    } else {
        std::error_code error;
        if (!state->directory.empty()) std::filesystem::remove_all(state->directory, error);
    }
    downloads.erase(download);
}

void download_failed(WKDownloadRef download, WKErrorRef, WKDataRef, const void* client_info)
{
    auto* state = static_cast<const DownloadState*>(client_info);
    std::error_code error;
    if (state->extension_package) {
        if (!state->directory.empty()) std::filesystem::remove_all(state->directory, error);
    } else if (!state->path.empty()) {
        std::filesystem::remove(state->path, error);
    }
    downloads.erase(download);
}

void attach_download(WKPageRef page, WKDownloadRef download)
{
    auto state = std::make_unique<DownloadState>();
    state->view = view_id(page);
    if (!state->view) return;
    state->client.base.version = 0;
    state->client.base.clientInfo = state.get();
    state->client.decideDestinationWithResponse = download_destination;
    state->client.didFinish = download_finished;
    state->client.didFailWithError = download_failed;
    WKDownloadSetClient(download, &state->client.base);
    downloads.emplace(download, std::move(state));
}

void action_became_download(WKPageRef page, WKNavigationActionRef, WKDownloadRef download, const void*)
{
    attach_download(page, download);
}

void response_became_download(WKPageRef page, WKNavigationResponseRef, WKDownloadRef download, const void*)
{
    attach_download(page, download);
}

const char* masked_user_agent(const std::string& uri)
{
    if (uri.find("addons.mozilla.org") != std::string::npos)
        return "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:152.0) Gecko/20100101 Firefox/152.0";
    if (uri.find("chromewebstore.google.com") != std::string::npos)
        return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
    if (uri.find("microsoftedge.microsoft.com/addons") != std::string::npos)
        return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36 Edg/152.0.0.0";
    if (uri.find("addons.opera.com") != std::string::npos)
        return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36 OPR/126.0.0.0";
    return nullptr;
}

void size_to_parent(WKViewRef view, HWND parent)
{
    RECT rect {};
    GetClientRect(parent, &rect);
    MoveWindow(WKViewGetWindow(view), 0, 0, rect.right - rect.left, rect.bottom - rect.top, TRUE);
}

UbarResult create_profile(uint64_t id, const UbarProfileConfigV1* config)
{
    if (!id || !config || profiles.contains(id) || !config->require_sandbox)
        return UBAR_INVALID_ARGUMENT;
    auto context = WKContextCreate();
    auto preferences = WKPreferencesCreate();
    auto userContentController = WKUserContentControllerCreate();
    char temporary[MAX_PATH] {};
    GetTempPathA(MAX_PATH, temporary);
    std::string rulePath = std::string(temporary) + "ubar-content-rules";
    auto rulePathString = make_string(rulePath);
    auto contentExtensionStore = WKUserContentExtensionStoreCreate(rulePathString);
    WKRelease(rulePathString);
    if (!context || !preferences || !userContentController || !contentExtensionStore)
        return UBAR_ENGINE_FAILURE;
    WKWebsiteDataStoreRef store = nullptr;
    if (config->kind == UBAR_PROFILE_PRIVATE) {
        store = WKWebsiteDataStoreCreateNonPersistentDataStore();
    } else {
        const auto data = copy_bytes(config->data_directory_utf8);
        const auto cache = copy_bytes(config->cache_directory_utf8);
        if (data.empty() && cache.empty()) {
            store = static_cast<WKWebsiteDataStoreRef>(WKRetain(WKWebsiteDataStoreGetDefaultDataStore()));
        } else {
            auto configuration = WKWebsiteDataStoreConfigurationCreate();
            if (!data.empty()) {
                auto directory = make_string(data);
                WKWebsiteDataStoreConfigurationSetGeneralStorageDirectory(configuration, directory);
                WKRelease(directory);
            }
            if (!cache.empty()) {
                auto directory = make_string(cache);
                WKWebsiteDataStoreConfigurationSetNetworkCacheDirectory(configuration, directory);
                WKRelease(directory);
            }
            store = WKWebsiteDataStoreCreateWithConfiguration(configuration);
            WKRelease(configuration);
        }
    }
    if (!store) {
        WKRelease(context);
        WKRelease(preferences);
        return UBAR_ENGINE_FAILURE;
    }
    WKPreferencesSetDeveloperExtrasEnabled(preferences, true);
    profiles.emplace(id, ProfileState {
        context, store, preferences, userContentController, contentExtensionStore
    });
    return UBAR_OK;
}

UbarResult destroy_profile(uint64_t id)
{
    auto found = profiles.find(id);
    if (found == profiles.end()) return UBAR_INVALID_ARGUMENT;
    for (const auto& [_, view] : views)
        if (view.profile == id) return UBAR_PERMISSION_DENIED;
    WKRelease(found->second.preferences);
    WKRelease(found->second.userContentController);
    WKRelease(found->second.contentExtensionStore);
    WKRelease(found->second.dataStore);
    WKRelease(found->second.context);
    profiles.erase(found);
    return UBAR_OK;
}

UbarResult create_view(uint64_t id, uint64_t profile, const UbarViewConfigV1* config,
                       const UbarCallbacksV1* callbacks)
{
    auto owner = profiles.find(profile);
    if (!id || owner == profiles.end() || !config || !config->native_parent || views.contains(id))
        return UBAR_INVALID_ARGUMENT;
    auto page = WKPageConfigurationCreate();
    WKPageConfigurationSetContext(page, owner->second.context);
    WKPageConfigurationSetWebsiteDataStore(page, owner->second.dataStore);
    WKPageConfigurationSetPreferences(page, owner->second.preferences);
    WKPageConfigurationSetUserContentController(page, owner->second.userContentController);
    RECT rect { 0, 0, static_cast<LONG>(config->width), static_cast<LONG>(config->height) };
    auto view = WKViewCreate(rect, page, static_cast<HWND>(config->native_parent));
    WKRelease(page);
    if (!view) return UBAR_ENGINE_FAILURE;
    ViewState state { profile, view, {} };
    if (callbacks) state.callbacks = *callbacks;
    views.emplace(id, state);
    size_to_parent(view, static_cast<HWND>(config->native_parent));
    WKPageSetCustomBackingScaleFactor(WKViewGetPage(view), config->device_scale);
    WKPageNavigationClientV3 navigation {};
    navigation.base.version = 3;
    navigation.decidePolicyForNavigationAction = navigation_action;
    navigation.decidePolicyForNavigationResponse = navigation_response;
    navigation.didStartProvisionalNavigation = navigation_started;
    navigation.didCommitNavigation = navigation_committed;
    navigation.didFinishNavigation = navigation_finished;
    navigation.webProcessDidCrash = web_process_crashed;
    navigation.webProcessDidTerminate = web_process_terminated;
    navigation.navigationActionDidBecomeDownload = action_became_download;
    navigation.navigationResponseDidBecomeDownload = response_became_download;
    WKPageSetPageNavigationClient(WKViewGetPage(view), &navigation.base);
    WKViewSetIsInWindow(view, config->initially_visible);
    ShowWindow(WKViewGetWindow(view), config->initially_visible ? SW_SHOW : SW_HIDE);
    return UBAR_OK;
}

UbarResult destroy_view(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    WKPageClose(WKViewGetPage(found->second.view));
    WKViewSetIsInWindow(found->second.view, false);
    WKRelease(found->second.view);
    if (found->second.headless_parent) DestroyWindow(found->second.headless_parent);
    views.erase(found);
    return UBAR_OK;
}

UbarResult create_headless_view(uint64_t id, uint64_t profile, const UbarCallbacksV1* callbacks)
{
    auto parent = CreateWindowExW(0, L"STATIC", L"uBar extension background", WS_POPUP,
                                  0, 0, 1, 1, nullptr, nullptr, GetModuleHandleW(nullptr), nullptr);
    if (!parent) return UBAR_ENGINE_FAILURE;
    UbarViewConfigV1 config { sizeof(UbarViewConfigV1), parent, 1, 1, 1., false };
    const auto result = create_view(id, profile, &config, callbacks);
    if (result != UBAR_OK) DestroyWindow(parent);
    else views.at(id).headless_parent = parent;
    return result;
}

UbarResult navigate(uint64_t id, UbarBytes value)
{
    auto found = views.find(id);
    const auto uri = copy_bytes(value);
    if (found == views.end() || uri.empty()) return UBAR_INVALID_ARGUMENT;
    auto page = WKViewGetPage(found->second.view);
    if (const char* userAgent = masked_user_agent(uri)) {
        auto value = WKStringCreateWithUTF8CString(userAgent);
        WKPageSetCustomUserAgent(page, value);
        WKRelease(value);
    } else {
        WKPageSetCustomUserAgent(page, nullptr);
    }
    auto url = WKURLCreateWithUTF8CString(uri.c_str());
    if (!url) return UBAR_INVALID_ARGUMENT;
    WKPageLoadURL(page, url);
    WKRelease(url);
    return UBAR_OK;
}

UbarResult set_visible(uint64_t id, bool visible)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    WKViewSetIsInWindow(found->second.view, visible);
    ShowWindow(WKViewGetWindow(found->second.view), visible ? SW_SHOW : SW_HIDE);
    return UBAR_OK;
}

UbarResult set_zoom(uint64_t id, double zoom)
{
    auto found = views.find(id);
    if (found == views.end() || zoom < .5 || zoom > 5.) return UBAR_INVALID_ARGUMENT;
    WKPageSetPageZoomFactor(WKViewGetPage(found->second.view), zoom);
    return UBAR_OK;
}

UbarResult suspend(uint64_t id) { return set_visible(id, false); }
UbarResult resume(uint64_t id) { return set_visible(id, true); }

UbarResult go_back(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    auto page = WKViewGetPage(found->second.view);
    if (!WKPageCanGoBack(page)) return UBAR_PERMISSION_DENIED;
    WKPageGoBack(page);
    return UBAR_OK;
}

UbarResult go_forward(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    auto page = WKViewGetPage(found->second.view);
    if (!WKPageCanGoForward(page)) return UBAR_PERMISSION_DENIED;
    WKPageGoForward(page);
    return UBAR_OK;
}

UbarResult reload(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    WKPageReload(WKViewGetPage(found->second.view));
    return UBAR_OK;
}

UbarResult stop(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    WKPageStopLoading(WKViewGetPage(found->second.view));
    return UBAR_OK;
}

void content_filter_compiled(
    WKUserContentFilterRef filter,
    WKUserContentExtensionStoreResult result,
    void* context)
{
    std::unique_ptr<uint64_t> profile(static_cast<uint64_t*>(context));
    auto found = profiles.find(*profile);
    if (found == profiles.end() || result != kWKUserContentExtensionStoreSuccess || !filter)
        return;
    WKUserContentControllerAddUserContentFilter(found->second.userContentController, filter);
}

UbarResult set_request_policy_json(uint64_t profile, UbarBytes value)
{
    auto found = profiles.find(profile);
    const auto json = copy_bytes(value);
    if (found == profiles.end() || json.empty()) return UBAR_INVALID_ARGUMENT;
    WKUserContentControllerRemoveAllUserContentFilters(found->second.userContentController);
    auto identifier = make_string("ubar-profile-" + std::to_string(profile));
    auto source = make_string(json);
    WKUserContentExtensionStoreCompile(
        found->second.contentExtensionStore, identifier, source,
        new uint64_t(profile), content_filter_compiled);
    WKRelease(source);
    WKRelease(identifier);
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
      if (!copy_bytes(world_utf8).empty()) {
        // Pinned WebKit's C API hard-codes evaluation to the page content world.
        // Keep this fail-closed until vendor/webkit/WINDOWS_ISOLATED_WORLDS.md
        // is implemented in the fork; a main-world fallback leaks extension APIs.
        return UBAR_UNSUPPORTED;
      }
      const auto script = copy_bytes(script_utf8);
      auto source = make_string(script);
      WKPageEvaluateJavaScriptInMainFrame(WKViewGetPage(found->second.view),
                                          source, nullptr, nullptr);
      WKRelease(source);
      return UBAR_OK;
    },
    create_headless_view
};

} // namespace

extern "C" UBAR_EXPORT UbarResult UBAR_CALL ubar_webkit_port_get_api(
    uint32_t requested_abi, const UbarWebKitPortApiV1** api_out)
{
    if (!api_out) return UBAR_INVALID_ARGUMENT;
    if (requested_abi != UBAR_WEBKIT_PORT_ABI_V1) return UBAR_ABI_MISMATCH;
    *api_out = &api;
    return UBAR_OK;
}
