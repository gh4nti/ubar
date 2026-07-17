# Windows C API isolated worlds

Pinned WebKit commit `3af9d4073d75a9b44668cebc03a41223dbef5745` cannot evaluate JavaScript in an isolated world through its public C API. `WKPage.h` only exports `WKPageEvaluateJavaScriptInMainFrame` and `WKPageEvaluateJavaScriptInFrame`; `WKPage.cpp` hard-codes both to `API::ContentWorld::pageContentWorldSingleton()`.

The Windows port must therefore keep returning `UBAR_UNSUPPORTED` when `world_utf8` is non-empty. Falling back to the page world would expose extension capabilities to website JavaScript.

## Smallest required WebKit fork seam

Add an opaque, reference-counted `WKContentWorldRef` C API backed by the existing `API::ContentWorld`:

```c
WK_EXPORT WKContentWorldRef WKContentWorldCreateWithName(WKStringRef name);
WK_EXPORT void WKPageEvaluateJavaScriptInFrameInContentWorld(
    WKPageRef page,
    WKFrameInfoRef frame,
    WKContentWorldRef world,
    WKStringRef script,
    void* context,
    WKPageEvaluateJavaScriptFunction callback);
```

Implementation requirements:

1. `WKContentWorldCreateWithName` returns `API::ContentWorld::sharedWorldWithName(toWTFString(name))` through WebKit's normal `toAPI` object bridge.
2. `WKPageEvaluateJavaScriptInFrameInContentWorld` copies the existing `WKPageEvaluateJavaScriptInFrame` implementation, but passes `*toImpl(world)` to `WebPageProxy::runJavaScriptInFrameInScriptWorld` instead of `API::ContentWorld::pageContentWorldSingleton()`.
3. The caller retains one world object per extension world name for the lifetime of its views. Do not create and immediately release a world for each script: bridge bootstrap and later content scripts must share the same JavaScript global.
4. Use the existing `WKRetain`/`WKRelease` ownership model. Reject null/empty world names so the page world remains an explicit separate path.

No WebProcess or JavaScriptCore change is needed. The pinned implementation already transports `ContentWorldData` through `WebPageProxy::runJavaScriptInFrameInScriptWorld` and executes it in the selected script world.

After that patch lands, the Windows uBar port should cache `WKContentWorldRef` by `world_utf8`, call the new function for non-empty worlds, and release cached worlds when the last profile/view using them is destroyed.
