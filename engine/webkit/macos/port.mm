#include "ubar_webkit_port.h"

#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>

#include <string>
#include <unordered_map>
#include <unordered_set>

@interface UbarExtensionMessageHandler : NSObject <WKScriptMessageHandler>
@end

namespace {

struct ProfileState {
    __strong WKProcessPool* processPool;
    __strong WKWebsiteDataStore* dataStore;
    __strong WKUserContentController* userContentController;
    __strong UbarExtensionMessageHandler* extensionMessageHandler;
};

@interface UbarNavigationDelegate : NSObject <WKNavigationDelegate, WKDownloadDelegate>
@property(nonatomic) uint64_t viewID;
@property(nonatomic) UbarCallbacksV1 callbacks;
@property(nonatomic, strong) NSMapTable<WKDownload*, NSURL*>* downloadDestinations;
@property(nonatomic, strong) NSHashTable<WKDownload*>* extensionDownloads;
@end

struct ViewState {
    uint64_t profile;
    __strong WKWebView* webView;
    __strong UbarNavigationDelegate* delegate;
    std::unordered_set<std::string> bridgeWorlds;
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

void clear_extension_worlds(uint64_t viewID)
{
    auto found = views.find(viewID);
    if (found == views.end()) return;
    auto owner = profiles.find(found->second.profile);
    if (owner == profiles.end()) return;
    for (const auto& world : found->second.bridgeWorlds) {
        NSString* name = [NSString stringWithUTF8String:world.c_str()];
        [owner->second.userContentController removeScriptMessageHandlerForName:@"ubarExtensionBridge"
            contentWorld:[WKContentWorld worldWithName:name]];
    }
    found->second.bridgeWorlds.clear();
}

@implementation UbarNavigationDelegate
- (BOOL)isExtensionResponse:(NSURLResponse*)response suggestedFilename:(NSString*)suggestedFilename
{
    NSString* mime = response.MIMEType.lowercaseString;
    NSString* extension = response.URL.pathExtension.lowercaseString;
    NSString* suggestedExtension = suggestedFilename.pathExtension.lowercaseString;
    return [mime isEqualToString:@"application/x-xpinstall"]
        || [mime isEqualToString:@"application/x-chrome-extension"]
        || [extension isEqualToString:@"xpi"] || [extension isEqualToString:@"crx"]
        || [suggestedExtension isEqualToString:@"xpi"] || [suggestedExtension isEqualToString:@"crx"];
}
- (NSString*)safeDownloadFilename:(NSString*)suggestedFilename
{
    NSString* filename = suggestedFilename.lastPathComponent.length
        ? suggestedFilename.lastPathComponent : @"download";
    NSMutableCharacterSet* unsafe = [[NSCharacterSet controlCharacterSet] mutableCopy];
    [unsafe addCharactersInString:@"/:\\?%*|\"<>"];
    filename = [[filename componentsSeparatedByCharactersInSet:unsafe] componentsJoinedByString:@"_"];
    filename = [filename stringByTrimmingCharactersInSet:
        [NSCharacterSet characterSetWithCharactersInString:@". "]];
    return filename.length ? filename : @"download";
}
- (NSURL*)collisionSafeDestinationInDirectory:(NSURL*)directory filename:(NSString*)filename
{
    NSFileManager* files = [NSFileManager defaultManager];
    NSURL* candidate = [directory URLByAppendingPathComponent:filename];
    if (![files fileExistsAtPath:candidate.path]) return candidate;
    NSString* extension = filename.pathExtension;
    NSString* stem = filename.stringByDeletingPathExtension;
    for (NSUInteger suffix = 1; suffix < 10000; ++suffix) {
        NSString* leaf = [NSString stringWithFormat:@"%@ (%lu)", stem, (unsigned long)suffix];
        if (extension.length) leaf = [leaf stringByAppendingPathExtension:extension];
        candidate = [directory URLByAppendingPathComponent:leaf];
        if (![files fileExistsAtPath:candidate.path]) return candidate;
    }
    NSString* leaf = [NSString stringWithFormat:@"%@-%f", stem, NSDate.timeIntervalSinceReferenceDate];
    if (extension.length) leaf = [leaf stringByAppendingPathExtension:extension];
    return [directory URLByAppendingPathComponent:leaf];
}
- (void)webView:(WKWebView*)webView didStartProvisionalNavigation:(WKNavigation*)navigation
{
    clear_extension_worlds(self.viewID);
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
- (void)webView:(WKWebView*)webView decidePolicyForNavigationAction:(WKNavigationAction*)action
    decisionHandler:(void (^)(WKNavigationActionPolicy))decisionHandler
{
    decisionHandler(action.shouldPerformDownload
        ? WKNavigationActionPolicyDownload : WKNavigationActionPolicyAllow);
}
- (void)webView:(WKWebView*)webView decidePolicyForNavigationResponse:(WKNavigationResponse*)response
    decisionHandler:(void (^)(WKNavigationResponsePolicy))decisionHandler
{
    NSString* disposition = [response.response isKindOfClass:[NSHTTPURLResponse class]]
        ? [(NSHTTPURLResponse*)response.response valueForHTTPHeaderField:@"Content-Disposition"] : nil;
    BOOL attachment = [disposition.lowercaseString containsString:@"attachment"];
    if (!response.canShowMIMEType || attachment
        || [self isExtensionResponse:response.response
            suggestedFilename:response.response.suggestedFilename])
        decisionHandler(WKNavigationResponsePolicyDownload);
    else
        decisionHandler(WKNavigationResponsePolicyAllow);
}
- (void)webView:(WKWebView*)webView navigationAction:(WKNavigationAction*)action
    didBecomeDownload:(WKDownload*)download
{
    download.delegate = self;
}
- (void)webView:(WKWebView*)webView navigationResponse:(WKNavigationResponse*)response
    didBecomeDownload:(WKDownload*)download
{
    download.delegate = self;
}
- (void)download:(WKDownload*)download decideDestinationUsingResponse:(NSURLResponse*)response
    suggestedFilename:(NSString*)suggestedFilename
    completionHandler:(void (^)(NSURL* destination))completionHandler
{
    const BOOL extensionPackage = [self isExtensionResponse:response suggestedFilename:suggestedFilename];
    NSURL* directory = nil;
    if (extensionPackage) {
        directory = [[NSURL fileURLWithPath:NSTemporaryDirectory() isDirectory:YES]
            URLByAppendingPathComponent:[[NSUUID UUID] UUIDString] isDirectory:YES];
    } else {
        directory = [[[NSFileManager defaultManager] URLsForDirectory:NSDownloadsDirectory
            inDomains:NSUserDomainMask] firstObject];
        if (!directory)
            directory = [[NSURL fileURLWithPath:NSHomeDirectory() isDirectory:YES]
                URLByAppendingPathComponent:@"Downloads" isDirectory:YES];
    }
    NSError* error = nil;
    if (![[NSFileManager defaultManager] createDirectoryAtURL:directory
        withIntermediateDirectories:!extensionPackage attributes:nil error:&error]) {
        completionHandler(nil);
        return;
    }
    NSURL* destination = extensionPackage
        ? [directory URLByAppendingPathComponent:@"package.xpi"]
        : [self collisionSafeDestinationInDirectory:directory
            filename:[self safeDownloadFilename:suggestedFilename]];
    [self.downloadDestinations setObject:destination forKey:download];
    if (extensionPackage) [self.extensionDownloads addObject:download];
    completionHandler(destination);
}
- (void)downloadDidFinish:(WKDownload*)download
{
    NSURL* destination = [self.downloadDestinations objectForKey:download];
    if (destination && [self.extensionDownloads containsObject:download])
        emit(self, UBAR_DOWNLOAD_REQUESTED, destination.path);
    [self.downloadDestinations removeObjectForKey:download];
    [self.extensionDownloads removeObject:download];
}
- (void)download:(WKDownload*)download didFailWithError:(NSError*)error resumeData:(NSData*)resumeData
{
    NSURL* destination = [self.downloadDestinations objectForKey:download];
    if (destination) {
        NSURL* cleanup = [self.extensionDownloads containsObject:download]
            ? destination.URLByDeletingLastPathComponent : destination;
        [[NSFileManager defaultManager] removeItemAtURL:cleanup error:nil];
    }
    [self.downloadDestinations removeObjectForKey:download];
    [self.extensionDownloads removeObject:download];
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
        profiles.emplace(id, ProfileState { pool, store, [WKUserContentController new],
            [UbarExtensionMessageHandler new] });
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
        delegate.downloadDestinations = [NSMapTable weakToStrongObjectsMapTable];
        delegate.extensionDownloads = [NSHashTable weakObjectsHashTable];
        if (callbacks)
            delegate.callbacks = *callbacks;
        webView.navigationDelegate = delegate;
        webView.hidden = !config->initially_visible;
        [parent addSubview:webView];
        views.emplace(id, ViewState { profile, webView, delegate, {} });
    }
    return UBAR_OK;
}

UbarResult destroy_view(uint64_t id)
{
    auto found = views.find(id);
    if (found == views.end())
        return UBAR_INVALID_ARGUMENT;
    clear_extension_worlds(id);
    [found->second.webView removeFromSuperview];
    views.erase(found);
    return UBAR_OK;
}

UbarResult create_headless_view(uint64_t id, uint64_t profile, const UbarCallbacksV1* callbacks)
{
    auto owner = profiles.find(profile);
    if (!id || owner == profiles.end() || views.contains(id)) return UBAR_INVALID_ARGUMENT;
    @autoreleasepool {
        WKWebViewConfiguration* config = [WKWebViewConfiguration new];
        config.processPool = owner->second.processPool;
        config.websiteDataStore = owner->second.dataStore;
        config.userContentController = owner->second.userContentController;
        config.preferences.javaScriptCanOpenWindowsAutomatically = NO;
        WKWebView* webView = [[WKWebView alloc] initWithFrame:NSZeroRect configuration:config];
        UbarNavigationDelegate* delegate = [UbarNavigationDelegate new];
        delegate.viewID = id;
        delegate.downloadDestinations = [NSMapTable weakToStrongObjectsMapTable];
        delegate.extensionDownloads = [NSHashTable weakObjectsHashTable];
        if (callbacks) delegate.callbacks = *callbacks;
        webView.navigationDelegate = delegate;
        views.emplace(id, ViewState { profile, webView, delegate, {} });
    }
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
    go_back, go_forward, reload, stop, set_request_policy_json,
    [](UbarView view, UbarBytes world_utf8, UbarBytes script_utf8) -> UbarResult {
      auto found = views.find(view);
      if (found == views.end()) {
        return UBAR_INVALID_ARGUMENT;
      }
      NSString* world = string_from_bytes(world_utf8);
      NSString* script = string_from_bytes(script_utf8);
      if (world.length) {
          auto owner = profiles.find(found->second.profile);
          if (owner == profiles.end()) return UBAR_INVALID_ARGUMENT;
          const std::string worldKey = world.UTF8String;
          if (found->second.bridgeWorlds.insert(worldKey).second)
              [owner->second.userContentController
                  addScriptMessageHandler:owner->second.extensionMessageHandler
                  contentWorld:[WKContentWorld worldWithName:world]
                  name:@"ubarExtensionBridge"];
          NSData* worldJSON = [NSJSONSerialization dataWithJSONObject:@[world] options:0 error:nil];
          NSString* quotedWorld = [[NSString alloc] initWithData:worldJSON encoding:NSUTF8StringEncoding];
          quotedWorld = [quotedWorld substringWithRange:NSMakeRange(1, quotedWorld.length - 2)];
          NSString* bridge = [NSString stringWithFormat:
              @"if(!globalThis.__ubarExtensionBridge)Object.defineProperty(globalThis,'__ubarExtensionBridge',{value:{postMessage(m){webkit.messageHandlers.ubarExtensionBridge.postMessage(JSON.stringify({world:%@,message:String(m)}));}},configurable:false});\n%@",
              quotedWorld, script];
          script = bridge;
      }
      WKContentWorld* content_world = world.length
          ? [WKContentWorld worldWithName:world]
          : [WKContentWorld pageWorld];
      [found->second.webView callAsyncJavaScript:script
                                      arguments:@{}
                                        inFrame:nil
                                 inContentWorld:content_world
                              completionHandler:nil];
      return UBAR_OK;
    },
    create_headless_view
};

} // namespace

@implementation UbarExtensionMessageHandler
- (void)userContentController:(WKUserContentController*)controller
      didReceiveScriptMessage:(WKScriptMessage*)message
{
    if (![message.body isKindOfClass:[NSString class]]) return;
    for (auto& [_, view] : views) {
        if (view.webView == message.webView) {
            emit(view.delegate, UBAR_EXTENSION_MESSAGE, (NSString*)message.body);
            return;
        }
    }
}
@end

extern "C" UBAR_EXPORT UbarResult ubar_webkit_port_get_api(
    uint32_t requested_abi, const UbarWebKitPortApiV1** api_out)
{
    if (!api_out) return UBAR_INVALID_ARGUMENT;
    if (requested_abi != UBAR_WEBKIT_PORT_ABI_V1) return UBAR_ABI_MISMATCH;
    *api_out = &api;
    return UBAR_OK;
}
