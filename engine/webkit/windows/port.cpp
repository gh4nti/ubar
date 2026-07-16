#include "ubar_webkit_port.h"

#include <windows.h>
#include <WebKit/WebKit2_C.h>

#include <string>
#include <memory>
#include <unordered_map>

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
};

std::unordered_map<uint64_t, ProfileState> profiles;
std::unordered_map<uint64_t, ViewState> views;

std::string copy_bytes(UbarBytes value)
{
    if (!value.data || !value.len) return {};
    return { reinterpret_cast<const char*>(value.data), value.len };
}

WKStringRef make_string(const std::string& value)
{
    return WKStringCreateWithUTF8CString(value.c_str());
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
    views.erase(found);
    return UBAR_OK;
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
    go_back, go_forward, reload, stop, set_request_policy_json
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
