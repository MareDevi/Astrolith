#!/usr/bin/env bash
# Shared packaging helpers: one metadata source, one binary version check.
set -euo pipefail

PACKAGING_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$PACKAGING_DIR/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
GENERATED_DIR="$PACKAGING_DIR/generated"
DESKTOP_ID="dev.astrolith.Astrolith"
BINARY_NAME="astrolith"
ICON_SIZES=(16 24 32 48 64 128 256 512)

fail() {
  echo "packaging: $*" >&2
  exit 1
}

require_tool() {
  local tool="$1" hint="${2:-}"
  command -v "$tool" >/dev/null 2>&1 || fail "missing prerequisite '$tool'. ${hint}"
}

workspace_version() {
  node "$PACKAGING_DIR/generate-metadata.mjs" --print-version
}

generate_metadata() {
  node "$PACKAGING_DIR/generate-metadata.mjs" --output "$GENERATED_DIR"
}

# Build the release binary and refuse to package when its version drifted from
# the generated metadata.
build_binary() {
  require_tool cargo "Install the pinned Rust toolchain with mise."
  require_tool node "Install the pinned Node.js toolchain with mise."
  generate_metadata
  cargo build --release --locked -p astrolith-desktop
  BINARY="$REPO_ROOT/target/release/astrolith-desktop"
  [[ -x "$BINARY" ]] || fail "the release binary was not built at $BINARY"
  node "$PACKAGING_DIR/generate-metadata.mjs" \
    --output "$GENERATED_DIR" \
    --check-binary "$BINARY"
}

# Install icons, desktop entry, AppStream metadata and the MDX MIME definition
# into a staging tree.
install_metadata() {
  local root="$1"
  local binary="${2:-$BINARY}"
  install -Dm755 "$binary" "$root/usr/bin/$BINARY_NAME"
  install -Dm644 "$GENERATED_DIR/astrolith.desktop" \
    "$root/usr/share/applications/$DESKTOP_ID.desktop"
  install -Dm644 "$GENERATED_DIR/astrolith.metainfo.xml" \
    "$root/usr/share/metainfo/$DESKTOP_ID.metainfo.xml"
  install -Dm644 "$GENERATED_DIR/astrolith.xml" \
    "$root/usr/share/mime/packages/$DESKTOP_ID.xml"
  local size
  for size in "${ICON_SIZES[@]}"; do
    install -Dm644 "$PACKAGING_DIR/icons/astrolith-$size.png" \
      "$root/usr/share/icons/hicolor/${size}x${size}/apps/$BINARY_NAME.png"
  done
  install -Dm644 "$PACKAGING_DIR/icons/astrolith.svg" \
    "$root/usr/share/icons/hicolor/scalable/apps/$BINARY_NAME.svg"
}

# run update-desktop-database and update-mime-database only when installed.
guarded_cache_update_script() {
  cat <<'SCRIPT'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database /usr/share/mime || true
fi
exit 0
SCRIPT
}
