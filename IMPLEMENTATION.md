# uBar implementation contract

This file is the delivery ledger. A requirement is not "done" until its acceptance test passes on every claimed platform.

Current measurements are recorded in [BENCHMARKS.md](BENCHMARKS.md).

## Product target

- Five representative modern tabs near 1 GiB resident memory on a normal desktop. This is a measured target, not a hard cap.
- Elastic resident ceiling: up to 25% of physical RAM when the machine has headroom.
- Constrained profile: 512 MiB Linux host, 2 GiB swap, one active tab, YouTube adaptive playback up to 720p when hardware decoding is available.
- Windows, macOS, and Linux; x86-64 and ARM64; one Rust browser-service base with thin OS-native shells.
- WebKit-based engine fork. No Chromium fork.

## Delivered foundation

- Persistent per-origin page zoom from 50% to 500%, including keyboard and visible Windows controls.
- Host-memory detection, 25% elastic ceiling, 1 GiB preferred desktop target, and a 512 MiB constrained policy.
- Shared profile/session/cache state instead of per-tab stores.
- Install/list/remove UI for downloaded XPI and CRX packages.
- Safe extension extraction: traversal, symlink, file-count, archive-size, and expanded-size checks.
- Native WebView2 extension loading on the current Windows prototype; WebKit content-script fallback elsewhere.
- Store identity policy for AMO, Chrome Web Store (Chrome 152), Edge Add-ons, and Opera Add-ons.
- Windows toolbar extension actions and non-empty extension management page.
- Bundled uBar Blocker: MV3 network rules on Windows and cosmetic rules on the WebKit fallback.
- Real Windows background-tab suspend/resume lifecycle using WebView2 `TrySuspend`, including reactivation race handling.
- Windows child-process working-set trimming after background suspension, allowing cold pages to move to the OS pagefile while preserving live WebView state.
- Benchmark restore probe switches to a suspended tab and records time until its second animation frame, then reports refaulted resident/private memory.
- Live Windows browser-process-tree resident-memory measurement in the built-in task manager.
- Reproducible `--benchmark-five-tabs` mode: five fixed modern sites, load/settle timeout, lifecycle data, machine policy, browser-tree process count, resident RAM, and private committed-memory JSON report. Override with exactly five comma-separated `UBAR_BENCHMARK_URLS`.
- Widevine component boundary: authorized local-path discovery, manifest/version validation, and native binary OS/architecture checks; WebView2-managed EME status on Windows.
- Private browsing uses a separate window/process and engine-private profile. Private tabs are excluded from history, session restoration, zoom persistence, and the download ledger.
- Linux privileged WebKit messages are accepted only from exact internal-page URIs; credential messages must match the current top-level origin.

## Required before an investor demo

### Shared engine and native UI

- Replace the GTK4/WebKitGTK and Windows/WebView2 split with one pinned WebKit fork and stable C ABI.
- Keep only native shells: WinUI 3 on Windows, SwiftUI/AppKit on macOS, GTK3 on Linux.
- Share browser services, profile format, networking, extension host, blocker, updater, telemetry controls, and tests.
- Hardware compositing and video decode validation on Intel, AMD, Apple Silicon, and common ARM Linux GPUs.

### Process and memory model

- One browser service plus bounded renderer groups; share network, GPU, font, image, script-bytecode, and immutable-resource caches.
- Keep OS/engine sandboxing. Sharing must use explicit brokered services and read-only mappings.
- Pressure ladder: trim caches, freeze background groups, checkpoint background groups, then checkpoint all hidden groups.
- WebKit renderer checkpoint/restore must preserve DOM, JS heap, history, form state, media position, timers, workers, and storage handles on disk.
- Crash-safe encrypted checkpoint journal, versioning, integrity checks, quota controls, and instant restore UX.
- Benchmarks: clean launch, five-tab set, 30-tab set, 720p playback, restore latency, tab-switch latency, energy use, and cold/warm cache.

### Extensions

- Firefox-first WebExtensions host covering every stable desktop namespace, event, permission, background page/worker, content script, popup, options page, sidebar, devtools, native messaging, commands, menus, downloads, cookies, tabs, windows, sessions, webRequest, declarativeNetRequest, proxy, privacy, and storage behavior.
- MV2 and MV3 lifecycle semantics plus a generated compatibility matrix against Firefox extension tests.
- Chrome manifest/API translation with explicit unsupported diagnostics; no silent no-op APIs.
- Signed-package verification, update manifests, permission prompts, per-extension process/accounting, disable/reload, crash recovery, and store-independent install flow.
- Store UAs must be maintained as data and updated by signed browser releases; transport and JS identity must agree before the first request.

### Blocking and privacy

- Integrate a maintained blocking engine and signed filter-list updater at the network-request layer.
- Cosmetic filtering, scriptlets, redirect resources, first/third-party rules, per-site disable, logger, custom lists, and regional lists.
- Partition or deliberately share state by documented policy; never leak private-mode state into the normal profile.

### Media and DRM

- EME/CDM host boundary supporting ClearKey in-tree and an authorized locally installed Widevine CDM.
- Widevine binaries must be obtained through an authorized license/download path and must never be copied from Firefox or redistributed without rights.
- CDM signature/version/architecture validation, encrypted CDM storage, output-protection reporting, crash isolation, revocation, and update handling.
- YouTube codec selection, MSE, WebAudio, fullscreen, Picture-in-Picture, captions, casting hooks, and hardware decode tests.

### Modern browser completeness

- Navigation, history, bookmarks, downloads, permissions, password/passkey integration, autofill, find, print/PDF, reader mode, profiles, private windows, tab groups, pinned tabs, sleeping-tab indicators, restore, sync, import/export, devtools, PWA install, notifications, WebRTC, screen capture, accessibility, internationalization, and automatic signed updates.
- TLS/certificate UI, Safe Browsing equivalent, phishing/malware download checks, site isolation policy, permission audit, and reproducible builds/SBOM.
- Floating Opera Air-inspired chrome, compact/comfortable modes, keyboard-only operation, touch, high contrast, reduced motion, screen readers, and platform-native menus/dialogs.

## Non-negotiable release gates

- No claim of full extension compatibility without passing the compatibility suite.
- No claim of live disk swapping until a killed renderer can restore an active test application without reload or observable state loss.
- No claim of 512 MiB YouTube support without measurement on the named hardware/image.
- No copied Widevine binary, bypassed signature, disabled sandbox, or misleading store identity outside the extension-store origins.
- ASan/UBSan, fuzzing of archives/protocol parsers, dependency audit, update rollback, crash reporting opt-in, and platform CI must pass.
