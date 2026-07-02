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
