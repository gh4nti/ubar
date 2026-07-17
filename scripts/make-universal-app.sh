#!/usr/bin/env sh
set -eu

x86_app="${1:?x86 app required}"
arm_app="${2:?arm app required}"
output="${3:?output app required}"
[ ! -e "$output" ] || { echo "output already exists: $output" >&2; exit 1; }
ditto "$arm_app" "$output"

find "$arm_app" -type f -print0 | while IFS= read -r -d '' arm_file; do
  relative="${arm_file#"$arm_app"/}"
  x86_file="$x86_app/$relative"
  output_file="$output/$relative"
  [ -f "$x86_file" ] || continue
  file "$arm_file" | grep -q 'Mach-O' || continue
  file "$x86_file" | grep -q 'Mach-O' || continue
  lipo -create "$x86_file" "$arm_file" -output "$output_file"
done
