# ubar

Tiny browser shell in C with GTK4 and WebKitGTK.

## Requirements

- `gcc`
- `gtk4`
- `webkitgtk-6.0`
- `pkg-config`

## Build

```sh
gcc main.c $(pkg-config --cflags --libs gtk4 webkitgtk-6.0) -o ubar
```

## Run

```sh
./ubar
```

Current app opens one window on `about:blank`.
