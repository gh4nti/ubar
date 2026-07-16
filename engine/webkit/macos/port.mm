#include "ubar_webkit_port.h"

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>

#include <string>
#include <unordered_map>

namespace {

struct ProfileState {
    __strong WKProcessPool* processPool;
    __strong WKWebsiteDataStore* dataStore;
    __strong WKUserContentController* userContentController;
};

@interface UbarNavigationDelegate : NSObject <WKNavigationDelegate>
@property(nonatomic) uint64_t viewID;
@property(nonatomic) UbarCallbacksV1 callbacks;
@end

struct ViewState {
    __strong WKWebView* webView;
    __strong UbarNavigationDelegate* delegate;
};

std::unordered_map<uint64_t, ProfileState> profiles;
std::unordered_map<uint64_t, ViewState> views;

void emit(UbarNavigationDelegate* delegate, UbarEventKind kind, NSString* text = nil, uint64_t value = 0)
{
    if (!delegate.callbacks.event)
        return;
    NSData* data = text ? [text dataUsingEncoding:NSUTF8StringEncoding] : nil;
    UbarEventV1 event {
        sizeof(UbarEventV1), kind, delegate.viewID,
        { static_cast<const uint8_t*>(data.bytes), data.length }, value
    };
    delegate.callbacks.event(delegate.callbacks.user_data, &event);
}

@implementation UbarNavigationDelegate
- (void)webView:(WKWebView*)webView didStartProvisionalNavigation:(WKNavigation*)navigation
{
    emit(self, UBAR_NAVIGATION_STARTED, webView.URL.absoluteString);
}
- (void)webView:(WKWebView*)webView didCommitNavigation:(WKNavigation*)navigation
{
    emit(self, UBAR_NAVIGATION_COMMITTED, webView.URL.absoluteString);
    emit(self, UBAR_URI_CHANGED, webView.URL.absoluteString);
}
- (void)webView:(WKWebView*)webView didFinishNavigation:(WKNavigation*)navigation
{
    emit(self, UBAR_NAVIGATION_FINISHED, webView.URL.absoluteString);
    emit(self, UBAR_TITLE_CHANGED, webView.title);
}
- (void)webViewWebContentProcessDidTerminate:(WKWebView*)webView
{
    emit(self, UBAR_RENDERER_CRASHED);
}
@end

NSString* string_from_bytes(UbarBytes value)
{
    if (!value.data || !value.len)
        return @"";
    return [[NSString alloc] initWithBytes:value.data length:value.len encoding:NSUTF8StringEncoding];
}

NSString* masked_user_agent(NSString* uri)
{
    if ([uri containsString:@"addons.mozilla.org"])
        return @"Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:152.0) Gecko/20100101 Firefox/152.0";
    if ([uri containsString:@"chromewebstore.google.com"])
        return @"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36";
    if ([uri containsString:@"microsoftedge.microsoft.com/addons"])
        return @"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36 Edg/152.0.0.0";
    if ([uri containsString:@"addons.opera.com"])
        return @"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36 OPR/126.0.0.0";
    return nil;
}

UbarResult create_profile(uint64_t id, const UbarProfileConfigV1* config)
{
    if (!id || !config || profiles.contains(id) || !config->require_sandbox)
        return UBAR_INVALID_ARGUMENT;
    @autoreleasepool {
        WKProcessPool* pool = [WKProcessPool new];
        WKWebsiteDataStore* store = config->kind == UBAR_PROFILE_PRIVATE
            ? [WKWebsiteDataStore nonPersistentDataStore]
            : [WKWebsiteDataStore defaultDataStore];
        profiles.emplace(id, ProfileState { pool, store, [WKUserContentController new] });
    }
    return UBAR_OK;
}

UbarResult destroy_profile(uint64_t id)
{
    if (!profiles.erase(id))
        return UBAR_INVALID_ARGUMENT;
    return UBAR_OK;
}

UbarResult create_view(uint64_t id, uint64_t profile, const UbarViewConfigV1* config,
                       const UbarCallbacksV1* callbacks)
{
    auto owner = profiles.find(profile);
    if (!id || owner == profiles.end() || !config || !config->native_parent || views.contains(id))
        return UBAR_INVALID_ARGUMENT;
    @autoreleasepool {
        auto* parent = (__bridge NSView*)config->native_parent;
        WKWebViewConfiguration* webConfig = [WKWebViewConfiguration new];
        webConfig.processPool = owner->second.processPool;
        webConfig.websiteDataStore = owner->second.dataStore;
        webConfig.userContentController = owner->second.userContentController;
        webConfig.preferences.javaScriptCanOpenWindowsAutomatically = NO;
        WKWebView* webView = [[WKWebView alloc] initWithFrame:parent.bounds configuration:webConfig];
        webView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
        UbarNavigationDelegate* delegate = [UbarNavigationDelegate new];
        delegate.viewID = id;
        if (callbacks)
            delegate.callbacks = *callbacks;
        webView.navigationDelegate = delegate;
        webView.hidden = !config->initially_visible;
        [parent addSubview:webView];
        views.emplace(id, ViewState { webView, delegate });
    }
    return UBAR_OK;
}

UbarResult destroy_view(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    [found->second.webView removeFromSuperview];
    views.erase(found);
    return UBAR_OK;
}

UbarResult navigate(uint64_t id, UbarBytes uri_bytes)
{
    auto found = views.find(id);
    NSString* uri = string_from_bytes(uri_bytes);
    if (found == views.end() || !uri.length)
        return UBAR_INVALID_ARGUMENT;
    found->second.webView.customUserAgent = masked_user_agent(uri);
    NSURL* url = [NSURL URLWithString:uri];
    if (!url)
        return UBAR_INVALID_ARGUMENT;
    [found->second.webView loadRequest:[NSURLRequest requestWithURL:url]];
    return UBAR_OK;
}

UbarResult set_visible(uint64_t id, bool visible)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    found->second.webView.hidden = !visible;
    return UBAR_OK;
}

UbarResult set_zoom(uint64_t id, double zoom)
{
    auto found = views.find(id);
    if (found == views.end() || zoom < .5 || zoom > 5.) return UBAR_INVALID_ARGUMENT;
    found->second.webView.pageZoom = zoom;
    return UBAR_OK;
}

UbarResult suspend(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    found->second.webView.hidden = YES;
    return UBAR_OK;
}

UbarResult resume(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    found->second.webView.hidden = NO;
    return UBAR_OK;
}

UbarResult go_back(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    if (![found->second.webView canGoBack]) return UBAR_PERMISSION_DENIED;
    [found->second.webView goBack];
    return UBAR_OK;
}

UbarResult go_forward(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    if (![found->second.webView canGoForward]) return UBAR_PERMISSION_DENIED;
    [found->second.webView goForward];
    return UBAR_OK;
}

UbarResult reload(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    [found->second.webView reload];
    return UBAR_OK;
}

UbarResult stop(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end()) return UBAR_INVALID_ARGUMENT;
    [found->second.webView stopLoading];
    return UBAR_OK;
}

UbarResult set_request_policy_json(uint64_t profile, UbarBytes value)
{
    auto found = profiles.find(profile);
    NSString* source = string_from_bytes(value);
    if (found == profiles.end() || !source.length) return UBAR_INVALID_ARGUMENT;
    [found->second.userContentController removeAllContentRuleLists];
    NSString* identifier = [NSString stringWithFormat:@"ubar-profile-%llu", profile];
    [[WKContentRuleListStore defaultStore]
        compileContentRuleListForIdentifier:identifier
        encodedContentRuleList:source
        completionHandler:^(WKContentRuleList* ruleList, NSError* error) {
            auto current = profiles.find(profile);
            if (current != profiles.end() && ruleList && !error)
                [current->second.userContentController addContentRuleList:ruleList];
        }];
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
    if (!api_out) return UBAR_INVALID_ARGUMENT;
    if (requested_abi != UBAR_WEBKIT_PORT_ABI_V1) return UBAR_ABI_MISMATCH;
    *api_out = &api;
    return UBAR_OK;
}
