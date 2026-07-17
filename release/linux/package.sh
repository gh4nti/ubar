#!/bin/sh
set -eu

stage="${1:?stage directory required}"
output="${2:?output directory required}"
appimage_arch="${3:?AppImage architecture required}"
deb_arch="${4:?Debian architecture required}"
rpm_arch="${5:?RPM architecture required}"
version="${6:?version required}"
work="$output/.package-work-$appimage_arch"
[ -d "$stage" ] || { echo "Linux stage missing: $stage" >&2; exit 1; }
[ ! -e "$work" ] || { echo "package work directory exists: $work" >&2; exit 1; }
mkdir -p "$output" "$work/root/usr/bin" "$work/root/usr/lib/ubar" \
  "$work/root/usr/share/applications" "$work/root/usr/share/icons/hicolor/scalable/apps"
install -m755 release/linux/ubar "$work/root/usr/bin/ubar"
install -m755 "$stage/ubar-gtk3-shell" "$work/root/usr/lib/ubar/ubar-gtk3-shell"
install -m755 "$stage/ubar-cdm-worker" "$work/root/usr/lib/ubar/ubar-cdm-worker"
install -m755 "$stage/ubar-xpi-verifier" "$work/root/usr/lib/ubar/ubar-xpi-verifier"
install -m755 "$stage/ubar-crx3-verifier" "$work/root/usr/lib/ubar/ubar-crx3-verifier"
cp -a "$stage"/*.so* "$work/root/usr/lib/ubar/"
install -m644 release/linux/dev.ghanti.ubar.desktop "$work/root/usr/share/applications/dev.ghanti.ubar.desktop"
install -m644 assets/branding/ubar.svg "$work/root/usr/share/icons/hicolor/scalable/apps/dev.ghanti.ubar.svg"

ditto_root="$work/AppDir"
cp -R "$work/root" "$ditto_root"
install -m755 release/linux/AppRun "$ditto_root/AppRun"
ln -s usr/share/applications/dev.ghanti.ubar.desktop "$ditto_root/dev.ghanti.ubar.desktop"
ln -s usr/share/icons/hicolor/scalable/apps/dev.ghanti.ubar.svg "$ditto_root/dev.ghanti.ubar.svg"
ARCH="$appimage_arch" appimagetool "$ditto_root" "$output/ubar-$appimage_arch.AppImage"

fpm -s dir -t deb -n ubar -v "$version" -a "$deb_arch" --depends libgtk-3-0 \
  -C "$work/root" -p "$output/ubar-${version}_${deb_arch}.deb" usr
fpm -s dir -t rpm -n ubar -v "$version" -a "$rpm_arch" --depends gtk3 \
  -C "$work/root" -p "$output/ubar-${version}.${rpm_arch}.rpm" usr

mkdir -p "$work/flatpak/stage"
cp "$stage/ubar-gtk3-shell" "$stage/ubar-cdm-worker" "$stage/ubar-xpi-verifier" \
  "$stage/ubar-crx3-verifier" "$work/flatpak/stage/"
cp -a "$stage"/*.so* "$work/flatpak/stage/"
if [ -f "$stage/libubar_widevine_adapter.so" ]; then
  cp "$stage/libubar_widevine_adapter.so" "$work/flatpak/stage/"
fi
cp release/linux/dev.ghanti.ubar.desktop "$work/flatpak/stage/"
cp assets/branding/ubar.svg "$work/flatpak/stage/dev.ghanti.ubar.svg"
cp release/linux/dev.ghanti.ubar.yml "$work/flatpak/"
flatpak-builder --force-clean --repo="$work/flatpak/repo" "$work/flatpak/build" \
  "$work/flatpak/dev.ghanti.ubar.yml"
flatpak build-bundle "$work/flatpak/repo" "$output/ubar-$appimage_arch.flatpak" dev.ghanti.ubar
