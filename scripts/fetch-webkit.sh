#!/usr/bin/env sh
set -eu

root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
metadata="$root/vendor/webkit/UPSTREAM.toml"
series="$root/vendor/webkit/patches/series"
destination="${1:-$root/vendor/webkit/src}"
repository="$(sed -n 's/^repository = "\([^"]*\)"/\1/p' "$metadata")"
commit="$(sed -n 's/^commit = "\([^"]*\)"/\1/p' "$metadata")"
[ -n "$repository" ] && [ -n "$commit" ] || { echo "Invalid WebKit metadata" >&2; exit 1; }
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

mkdir -p "$destination"
if [ ! -d "$destination/.git" ]; then
  git -C "$destination" init
  git -C "$destination" remote add origin "$repository"
fi
git -C "$destination" fetch --depth=1 origin "$commit"
git -C "$destination" checkout --detach FETCH_HEAD
actual="$(git -C "$destination" rev-parse HEAD)"
[ "$actual" = "$commit" ] || { echo "WebKit pin mismatch: $actual" >&2; exit 1; }

while IFS=' ' read -r expected patch; do
  case "$expected" in ''|'#'*) continue ;; esac
  file="$root/vendor/webkit/patches/$patch"
  [ -f "$file" ] || { echo "Missing WebKit patch: $patch" >&2; exit 1; }
  actual_hash="$(sha256 "$file")"
  [ "$actual_hash" = "$expected" ] || { echo "WebKit patch hash mismatch: $patch" >&2; exit 1; }
  git -C "$destination" am --3way "$file"
done < "$series"

(cd "$destination" && find . -type f \( -iname 'copying*' -o -iname 'license*' \) -print \
  | sed 's#^\./##' | LC_ALL=C sort) > "$destination/.ubar-license-files"
series_hash="$(sha256 "$series")"
printf 'upstream=%s\npatchset_sha256=%s\n' "$commit" "$series_hash" \
  > "$destination/.ubar-source-state"
printf 'WebKit prepared at %s (patchset %s)\n' "$commit" "$series_hash"
