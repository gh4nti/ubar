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

## Build

```sh
make
```

## Run

```sh
./build/ubar
```

Run from repo root so bundled assets under `build/assets/` are available.
