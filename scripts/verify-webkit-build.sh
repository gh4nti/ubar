#!/usr/bin/env sh
set -eu

root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
build="${1:?usage: verify-webkit-build.sh BUILD_DIRECTORY}"
state="$build/.ubar-source-state"
[ -f "$state" ] || { echo "Pinned WebKit build state missing: $state" >&2; exit 1; }
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}
expected_commit="$(sed -n 's/^commit = "\([^"]*\)"/\1/p' "$root/vendor/webkit/UPSTREAM.toml")"
expected_patchset="$(sha256 "$root/vendor/webkit/patches/series")"
actual_commit="$(sed -n 's/^upstream=//p' "$state")"
actual_patchset="$(sed -n 's/^patchset_sha256=//p' "$state")"
[ "$actual_commit" = "$expected_commit" ] || { echo "WebKit build commit mismatch" >&2; exit 1; }
[ "$actual_patchset" = "$expected_patchset" ] || { echo "WebKit build patchset mismatch" >&2; exit 1; }
