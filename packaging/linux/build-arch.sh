#!/usr/bin/env bash
# Build an Arch Linux package with makepkg from the same generated metadata.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require_tool makepkg \
  "Install makepkg (Arch Linux: pacman -S base-devel) or run this task on an Arch system."
require_tool cp "Install coreutils."

case "$(uname -m)" in
  x86_64) ARCH="x86_64" ;;
  aarch64) ARCH="aarch64" ;;
  *) fail "unsupported architecture for an Arch package: $(uname -m)" ;;
esac

build_binary
VERSION="$(workspace_version)"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
ROOT="$WORK/root"
mkdir -p "$ROOT"

install_metadata "$ROOT" "$BINARY"

cat >"$WORK/astrolith.install" <<'INSTALL'
post_install() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
  fi
  if command -v update-mime-database >/dev/null 2>&1; then
    update-mime-database /usr/share/mime || true
  fi
}

post_upgrade() {
  post_install
}

pre_remove() {
  post_install
}
INSTALL

cat >"$WORK/PKGBUILD" <<PKGBUILD
pkgname=astrolith
pkgver=$VERSION
pkgrel=1
pkgdesc='Local-first editor for Astro Markdown and MDX content'
arch=('$ARCH')
url='https://astrolith.invalid/'
license=('unknown')
depends=('hicolor-icon-theme')
install=astrolith.install

package() {
  cp -a "$ROOT/." "\$pkgdir/"
}
PKGBUILD

(cd "$WORK" && PACKAGER="${PACKAGER:-Astrolith Contributors <maintainers@astrolith.invalid>}" \
  makepkg --nodeps --noconfirm --force --clean)

PACKAGE="$(find "$WORK" -maxdepth 1 -type f -name 'astrolith-*.pkg.tar.*' ! -name '*.sig' -print -quit)"
[[ -n "$PACKAGE" ]] || fail "makepkg did not produce a package"

mkdir -p "$DIST_DIR"
OUTPUT="$DIST_DIR/astrolith-${VERSION}-1-${ARCH}.pkg.tar${PACKAGE##*.pkg.tar}"
cp "$PACKAGE" "$OUTPUT"
echo "packaging: wrote $OUTPUT"
