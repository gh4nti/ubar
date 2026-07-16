# Pinned WebKit source

WebKit source is fetched into `vendor/webkit/src` and is intentionally not committed. `UPSTREAM.toml` is the immutable source pin. Local uBar changes belong in `vendor/webkit/patches` as numbered patches; editing fetched source without updating the patch queue is forbidden.

```powershell
./scripts/fetch-webkit.ps1
```

The upstream Windows port currently documents 64-bit Windows support. Windows ARM64 remains a uBar porting task and cannot be certified from the x64 build.
