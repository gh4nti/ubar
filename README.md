# ubar

Tiny browser shell in C with GTK4 and WebKitGTK.

## Requirements

- `gcc`
- `gtk4`
- `webkitgtk-6.0`
- `pkg-config`

## Layout

- `src/` application code
- `include/` public headers
- `assets/newtab/` new tab page HTML/CSS/JS
- `build/` compiled output

## Permanent browser data

History, bookmarks, and the homepage setting are stored permanently in the user data directory as `ubar/state.ini`. Saves are atomic and a `state.ini.bak` backup is kept, so a crash during write should not wipe the library. History entries are no longer capped at 200.

## Build

```sh
make
```

## Run

```sh
./build/ubar
```

Run from repo root so bundled assets under `build/assets/` are available.


## Browser UI and settings changes

- Tabs are on the top strip, with the address bar moved below the tabs.
- Back, forward, and reload live on the left side of the address bar.
- Bookmark and menu live on the right side of the address bar.
- The native title bar is hidden and window controls are placed beside the tab strip.
- Settings now has Chrome-inspired sections for Startup, Privacy and security, Site settings, Appearance, and Library.
- Site settings are stored per origin. Defaults and per-site overrides are saved in `state.ini` for notifications, location, cookies, camera, microphone, JavaScript, and pop-ups.

Note: WebKit permission prompts such as location/notifications/camera are enforced through the saved allow/block rules. Cookie, JavaScript, and pop-up entries are currently stored and shown in Settings as browser preferences; deeper WebKit enforcement can be wired in later per WebKit API support.
