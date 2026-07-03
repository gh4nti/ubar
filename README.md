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

Unpacked Firefox-style extensions load from `~/.local/share/ubar/extensions/<name>/` (a folder containing `manifest.json`). Supported: `content_scripts` (js + css, `matches`, `run_at`) with a minimal `browser.*` shim (`storage.local`, `runtime.getURL`/`getManifest`). Background pages, popups, and `webRequest` are not supported.

## Shortcuts

Ctrl+T new tab, Ctrl+Shift+T reopen closed, Ctrl+W close, Ctrl+L address bar, Ctrl+F find in page, Ctrl+R reload, Ctrl+D bookmark, Ctrl+H history, Ctrl+1..9 switch tab, Ctrl+&plus;/&minus;/0 zoom, F12 devtools.
