# Pinned WebKit source

WebKit source is fetched into `vendor/webkit/src` and is intentionally not committed. `UPSTREAM.toml` is the immutable source pin. Local uBar changes belong in `vendor/webkit/patches` as numbered `git format-patch` files listed with SHA-256 in `patches/series`; editing fetched source without updating the patch queue is forbidden. Fetch writes `.ubar-source-state` and a complete discovered license-file inventory used by release runners.

```powershell
./scripts/fetch-webkit.ps1
```

The upstream Windows port currently documents 64-bit Windows support. Windows ARM64 remains a uBar porting task and cannot be certified from the x64 build.
