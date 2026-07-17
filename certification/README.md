# uBar certification evidence

Certification is evidence, not a build label. Every release target in
`release/targets.toml` needs a signed release manifest and evidence from its
matching physical runner in `matrix.toml`.

The runner records these required environment values before invoking
`ubar-release-tool certify`:

- `UBAR_CERT_RAM_BYTES`, `UBAR_CERT_CPU`, `UBAR_CERT_GPU`, `UBAR_CERT_GPU_DRIVER`
- `UBAR_CERT_RENDERER_ESCAPE=blocked`
- `UBAR_CERT_CROSS_SITE_STORAGE=isolated`
- `UBAR_CERT_PRIVATE_PERSISTENCE=none`
- `UBAR_CERT_PRIVILEGED_IPC=authenticated`
- `UBAR_CERT_FIVE_TAB_RSS_BYTES`
- `UBAR_CERT_YOUTUBE_720P_AVG_FPS`
- `UBAR_CERT_YOUTUBE_720P_DROPPED_PERCENT`
- `UBAR_CERT_HARDWARE_DECODE=true|false`

Example:

```sh
cargo run -p ubar-release-tool -- certify \
  --manifest dist/release-manifest.json \
  --target aarch64-unknown-linux-gnu \
  --output dist/certification.json
```

Missing measurements fail. The tool never substitutes synthetic evidence.
Production Widevine certification remains separate and requires licensed
Google test credentials. Release runners may expose
`UBAR_WIDEVINE_ADAPTER_PATH` to package an authorized adapter implementing
`include/ubar_cdm_adapter.h`. Widevine binaries, credentials, and licenses
stay outside repository. Windows builds compile and sign their AppContainer
broker from `platform/windows/cdm-broker`; no external broker binary is trusted.
