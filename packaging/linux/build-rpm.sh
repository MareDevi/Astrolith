#!/usr/bin/env bash
# Build an RPM with rpmbuild from the same generated metadata.
set -euo pipefail
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

require_tool rpmbuild \
  "Install rpmbuild (Fedora: rpm-build; Debian: rpm) or run this task in the release workflow."

[[ "$(uname -m)" == "x86_64" ]] || fail "this RPM recipe only supports x86_64"

build_binary
VERSION="$(workspace_version)"

TOP="$(mktemp -d)"
trap 'rm -rf "$TOP"' EXIT
mkdir -p "$TOP/SPECS" "$TOP/BUILD" "$TOP/BUILDROOT" "$TOP/RPMS" "$TOP/SRPMS"
SPEC="$TOP/SPECS/astrolith.spec"

{
  cat <<SPEC
Name: astrolith
Version: $VERSION
Release: 1%{?dist}
Summary: Local-first editor for Astro Markdown and MDX content
License: UNLICENSED
URL: https://astrolith.invalid/
BuildArch: x86_64
Requires: hicolor-icon-theme

%description
Astrolith edits the Markdown and MDX entries of local Astro projects.
It validates frontmatter against collection schemas and builds MDX component
imports without sending content anywhere.

%prep

%build

%install
install -Dm755 "$BINARY" %{buildroot}/usr/bin/$BINARY_NAME
install -Dm644 "$GENERATED_DIR/astrolith.desktop" %{buildroot}/usr/share/applications/$DESKTOP_ID.desktop
install -Dm644 "$GENERATED_DIR/astrolith.metainfo.xml" %{buildroot}/usr/share/metainfo/$DESKTOP_ID.metainfo.xml
install -Dm644 "$GENERATED_DIR/astrolith.xml" %{buildroot}/usr/share/mime/packages/$DESKTOP_ID.xml
SPEC
  for size in "${ICON_SIZES[@]}"; do
    printf 'install -Dm644 "%s/icons/astrolith-%s.png" %%{buildroot}/usr/share/icons/hicolor/%sx%s/apps/%s.png\n' \
      "$PACKAGING_DIR" "$size" "$size" "$size" "$BINARY_NAME"
  done
  printf 'install -Dm644 "%s/icons/astrolith.svg" %%{buildroot}/usr/share/icons/hicolor/scalable/apps/%s.svg\n' \
    "$PACKAGING_DIR" "$BINARY_NAME"
  cat <<SPEC

%files
/usr/bin/$BINARY_NAME
/usr/share/applications/$DESKTOP_ID.desktop
/usr/share/metainfo/$DESKTOP_ID.metainfo.xml
/usr/share/mime/packages/$DESKTOP_ID.xml
/usr/share/icons/hicolor/*/apps/$BINARY_NAME.*

%post
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || :
fi
if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database /usr/share/mime || :
fi

%preun
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || :
fi
if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database /usr/share/mime || :
fi

%changelog
* $(date -u +'%a %b %d %Y') Astrolith Contributors <maintainers@astrolith.invalid> - $VERSION-1
- Automated package build.
SPEC
} >"$SPEC"

rpmbuild -bb \
  --define "_topdir $TOP" \
  --define "_build_id_links none" \
  "$SPEC"

mkdir -p "$DIST_DIR"
OUTPUT="$DIST_DIR/astrolith-${VERSION}-1.x86_64.rpm"
cp "$TOP/RPMS/x86_64/"*.rpm "$OUTPUT"
echo "packaging: wrote $OUTPUT"
