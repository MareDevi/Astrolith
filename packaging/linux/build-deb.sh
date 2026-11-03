#!/usr/bin/env bash
# Build a Debian package with ar and tar only; no dpkg-deb required.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require_tool ar "Install binutils."
require_tool tar "Install tar."
require_tool install "Install coreutils."

case "$(uname -m)" in
  x86_64) DEB_ARCH="amd64" ;;
  aarch64 | arm64) DEB_ARCH="arm64" ;;
  *) fail "unsupported architecture for a Debian package: $(uname -m)" ;;
esac

build_binary
VERSION="$(workspace_version)"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ROOT="$STAGE/root"
CONTROL_ROOT="$STAGE/control"
CONTROL="$CONTROL_ROOT/DEBIAN"
PKG="$STAGE/pkg"
mkdir -p "$ROOT" "$CONTROL" "$PKG"

install_metadata "$ROOT" "$BINARY"

INSTALLED_SIZE="$(du -sk "$ROOT" | cut -f1)"
{
  printf 'Package: astrolith\n'
  printf 'Version: %s\n' "$VERSION"
  printf 'Architecture: %s\n' "$DEB_ARCH"
  printf 'Maintainer: Astrolith Contributors <maintainers@astrolith.invalid>\n'
  printf 'Section: editors\n'
  printf 'Priority: optional\n'
  printf 'Installed-Size: %s\n' "$INSTALLED_SIZE"
  printf 'Description: Local-first editor for Astro Markdown and MDX content\n'
  printf ' Astrolith edits the Markdown and MDX entries of local Astro projects.\n'
  printf ' It validates frontmatter against collection schemas and builds MDX\n'
  printf ' component imports without sending content anywhere.\n'
} >"$CONTROL/control"
guarded_cache_update_script >"$CONTROL/postinst"
guarded_cache_update_script >"$CONTROL/prerm"
chmod 755 "$CONTROL/postinst" "$CONTROL/prerm"

(cd "$ROOT" && tar --sort=name --owner=0 --group=0 --numeric-owner -czf "$PKG/data.tar.gz" .)
(cd "$CONTROL_ROOT" && tar --sort=name --owner=0 --group=0 --numeric-owner -czf "$PKG/control.tar.gz" ./DEBIAN)
printf '2.0\n' >"$PKG/debian-binary"

mkdir -p "$DIST_DIR"
OUTPUT="$DIST_DIR/astrolith_${VERSION}_${DEB_ARCH}.deb"
(cd "$PKG" && ar rcs "$OUTPUT" debian-binary control.tar.gz data.tar.gz)
echo "packaging: wrote $OUTPUT"
