# ubar

Tiny browser shell in Rust with GTK4 and WebKitGTK.

## Requirements

- `rust`
- `cargo`
- `gtk4`
- `webkitgtk-6.0`
- `pkg-config`

## Layout

- `src/` Rust application code
- `assets/newtab/` new tab page HTML/CSS/JS
- `assets/pages/` internal history, bookmarks, and settings pages
- `build/` runtime output

## Permanent browser data

History, bookmarks, and the homepage setting are stored in the user data directory as `ubar/state.json`.

## Build

```sh
make
```

## Run

```sh
./build/linux/ubar
```

Assets are copied to `build/linux/assets/` so the executable can load the internal pages beside itself.

## Extensions

Two ways to install:

1. **From the stores**: visit [addons.mozilla.org](https://addons.mozilla.org) or [chromewebstore.google.com](https://chromewebstore.google.com). ubar spoofs the matching browser UA on those sites and shows a floating "Install in ubar" button; clicking it downloads the `.xpi`/`.crx` and installs it automatically. The Extensions page opens when done.
2. **Manually**: drop an unpacked extension folder (containing `manifest.json`) into `~/.local/share/ubar/extensions/<name>/` and restart.

Manage installed extensions from menu -> Extensions.

Supported: `content_scripts` (js + css, `matches`, `run_at`) with a minimal `browser.*`/`chrome.*` shim (`storage.local`, `runtime.getURL`/`getManifest`). Background pages/service workers, popups, and `webRequest` are not supported -- extensions that rely only on those install but do nothing.

## Shortcuts

Ctrl+T new tab, Ctrl+Shift+T reopen closed, Ctrl+W close, Ctrl+L address bar, Ctrl+F find in page, Ctrl+R reload, Ctrl+D bookmark, Ctrl+H history, Ctrl+1..9 switch tab, Ctrl+&plus;/&minus;/0 zoom, F12 devtools.
