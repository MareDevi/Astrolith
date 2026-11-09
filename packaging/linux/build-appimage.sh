#!/usr/bin/env bash
# Build an AppImage with linuxdeploy and appimagetool.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require_tool linuxdeploy \
  "Install linuxdeploy (for example the continuous AppImage from linuxdeploy/linuxdeploy) or run this task in the release workflow."
require_tool appimagetool \
  "Install appimagetool (AppImageKit releases) or run this task in the release workflow."

[[ "$(uname -m)" == "x86_64" ]] || fail "this AppImage recipe only supports x86_64"

build_binary
VERSION="$(workspace_version)"

APPDIR="$DIST_DIR/AppDir"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/metainfo" "$APPDIR/usr/share/mime/packages" \
  "$APPDIR/usr/share/icons/hicolor"
install_metadata "$APPDIR" "$BINARY"
install_licenses "$APPDIR/usr/share/licenses/$BINARY_NAME"
install -Dm644 "$GENERATED_DIR/astrolith.desktop" "$APPDIR/$DESKTOP_ID.desktop"
install -Dm644 "$PACKAGING_DIR/icons/astrolith-512.png" "$APPDIR/astrolith.png"
ln -s "usr/bin/$BINARY_NAME" "$APPDIR/AppRun"

ARCH=x86_64 linuxdeploy \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/$BINARY_NAME" \
  --desktop-file "$APPDIR/$DESKTOP_ID.desktop" \
  --icon-file "$APPDIR/astrolith.png"

mkdir -p "$DIST_DIR"
OUTPUT="$DIST_DIR/Astrolith-${VERSION}-x86_64.AppImage"
ARCH=x86_64 VERSION="$VERSION" appimagetool "$APPDIR" "$OUTPUT"
echo "packaging: wrote $OUTPUT"
