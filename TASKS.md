# uBar major-engine tasks

This ledger separates code completion from external licensing and hardware certification. A task closes only when its acceptance commands and evidence pass.

## ENG — shared WebKit engine

- [x] ENG-001: Define a versioned, platform-neutral C ABI for profiles, views, navigation, zoom, visibility, suspension, request policy, events, and CDM registration.
- [x] ENG-002: Add a shared Rust browser-core with explicit normal/private profiles and security defaults.
- [ ] ENG-003: Import a pinned WebKit fork under `vendor/webkit` with upstream commit, patch queue, license inventory, and reproducible fetch script.
- [ ] ENG-004: Implement the ABI adapter against WebKit UIProcess/WebProcess on Windows, macOS, and Linux.
- [ ] ENG-005: Implement renderer-group checkpoint/restore with encrypted disk journal and ABI-version rejection.
- [ ] ENG-006: Share network, GPU, font, immutable-resource, bytecode, and image caches through brokered services.
- [ ] ENG-007: Enforce renderer sandbox, broker allowlists, site isolation, top-level storage partitioning, and private-profile non-persistence.

Acceptance: ABI conformance tests, Web Platform Tests, process escape tests, cross-origin storage tests, crash restore, and five-tab benchmark all pass.

## SHELL — native platform UI

- [ ] SHELL-001: WinUI 3 shell with native title bar, tabs, omnibox, menus, dialogs, accessibility, touch, and engine-ABI hosting.
- [ ] SHELL-002: SwiftUI/AppKit shell with equivalent behavior and engine-ABI hosting.
- [ ] SHELL-003: GTK3 shell with equivalent behavior and engine-ABI hosting; remove GTK4 runtime dependency after parity.
- [ ] SHELL-004: Generate shared command/event bindings from one schema and run shell parity tests.

Acceptance: platform accessibility tools report no critical failures; keyboard, touch, high contrast, reduced motion, menus, downloads, permissions, and private windows pass the shared UI suite.

## EXT — Firefox-first WebExtensions

- [x] EXT-001: Generate the supported namespace/event/permission registry from Firefox WebExtensions schemas.
- [ ] EXT-002: Implement isolated extension worlds, background pages/workers, structured messaging, ports, lifecycle, and permission prompts.
- [ ] EXT-003: Implement stable Firefox desktop namespaces and semantics; no silent no-op methods.
- [ ] EXT-004: Add MV2/MV3 Chrome translation and explicit incompatibility diagnostics.
- [ ] EXT-005: Verify XPI/CRX signatures, updates, revocation, process accounting, crash recovery, and private-mode policy.
- [ ] EXT-006: Run Firefox extension tests plus uBlock-class integration tests on every supported target.

Acceptance: generated compatibility report has no unclassified stable Firefox API and required reference extensions pass install, update, popup, background, content-script, storage, and network tests.

## DRM — EME and licensed Widevine host

- [ ] DRM-001: Implement W3C EME session/state machine and ClearKey CDM in-tree.
- [ ] DRM-002: Implement sandboxed CDM process and versioned host RPC.
- [ ] DRM-003: Integrate authorized Widevine CDM adapter using licensed headers/binaries only.
- [ ] DRM-004: Add signature, architecture, revocation, output-protection, persistent-license, crash, and update handling.
- [ ] DRM-005: Pass encrypted MSE playback, suspend/resume, fullscreen, PiP, and hardware decode tests.

External gate: production Widevine cannot close until the project receives Google CDM distribution/host authorization and test credentials.

## REL — platform builds and certification

- [ ] REL-001: Windows x86-64/ARM64 CI, MSIX signing, update rollback, ASan, and accessibility tests.
- [ ] REL-002: macOS x86-64/ARM64 CI, universal app, hardened runtime, notarization, sandbox entitlements, and accessibility tests.
- [ ] REL-003: Linux x86-64/ARM64 CI, GTK3/WebKit packages, AppImage/Flatpak/deb/rpm, seccomp, and accessibility tests.
- [ ] REL-004: Reproducible builds, SBOM, dependency/license audit, fuzzing corpus, signed updates, and provenance.
- [ ] REL-005: Hardware matrix certification for GPU compositing, video decode, suspend/restore, power, memory, and 512 MiB Linux.

External gate: signing identities, notarization credentials, hardware runners, and DRM authorization must be supplied through CI secrets; none may be committed.
