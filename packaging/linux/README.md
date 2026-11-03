# Linux packaging

Astrolith ships one binary and one metadata source. Everything under
`generated/` is produced from `templates/` plus the Cargo workspace version by
`generate-metadata.mjs`; never edit or commit generated files.

```sh
mise run package:metadata   # regenerate desktop, AppStream and MIME metadata
mise run package:validate   # desktop-file-validate + appstreamcli validate
mise run package:deb        # dist/astrolith_<version>_<arch>.deb
mise run package:appimage   # dist/Astrolith-<version>-x86_64.AppImage
mise run package:rpm        # dist/astrolith-<version>-1.x86_64.rpm
```

Every build script rebuilds the release binary and refuses to package when
`astrolith --version` disagrees with the generated metadata version.

## Metadata

| File | Installed as | Purpose |
|---|---|---|
| `generated/astrolith.desktop` | `/usr/share/applications/dev.astrolith.Astrolith.desktop` | Application entry, MIME and `astrolith://` scheme association |
| `generated/astrolith.metainfo.xml` | `/usr/share/metainfo/dev.astrolith.Astrolith.metainfo.xml` | AppStream component, launchable id, provided media types, release entry |
| `generated/astrolith.xml` | `/usr/share/mime/packages/dev.astrolith.Astrolith.xml` | Declares `text/mdx` (glob `*.mdx`, subclass of `text/markdown`) |

`text/markdown` itself is not redefined; the desktop entry associates it
directly. The `astrolith` scheme is registered through
`x-scheme-handler/astrolith` and routed by `url_scheme.rs`.

Icons come from `icons/` and install into `hicolor` at 16, 24, 32, 48, 64, 128,
256 and 512 pixels plus the scalable SVG.

## Artifact contents

The Debian package is assembled with `ar` and `tar`; `dpkg-deb` is not needed.
It contains the binary, desktop entry, AppStream metadata, MIME definition and
all icons. Its `postinst` and `prerm` run `update-desktop-database` and
`update-mime-database` only when those commands exist.

The AppImage recipe stages the same files into an AppDir and delegates to
`linuxdeploy` and `appimagetool`. The RPM recipe generates a spec, builds with
`rpmbuild`, and runs the same guarded cache updates in `%post`/`%preun`.

## Update manifest

The application checks for updates only when the build injected both
`ASTROLITH_UPDATE_MANIFEST_URL` (HTTPS) and `ASTROLITH_UPDATE_PUBLIC_KEY` (base64
of the raw 32-byte Ed25519 public key). Without them the Check for updates
command is unavailable and the status surface explains that updates are not
configured.

`sign-manifest.mjs` produces the envelope the application verifies:

```sh
export ASTROLITH_UPDATE_SIGNING_KEY="$(base64 -w0 update-key.pkcs8.der)"
node packaging/linux/sign-manifest.mjs \
  --version 0.1.0 \
  --notes-file release-notes.md \
  --page https://example.invalid/astrolith/releases/v0.1.0 \
  --artifact linux-x86_64-appimage \
      dist/Astrolith-0.1.0-x86_64.AppImage \
      https://example.invalid/astrolith/releases/download/v0.1.0/Astrolith-0.1.0-x86_64.AppImage \
  --out dist/astrolith-update-manifest.json
```

Generate the key material outside the repository:

```sh
openssl genpkey -algorithm ED25519 -out update-key.pem
openssl pkcs8 -topk8 -nocrypt -in update-key.pem -outform DER | base64 -w0
# public key for ASTROLITH_UPDATE_PUBLIC_KEY
openssl pkey -in update-key.pem -pubout -outform DER | tail -c 32 | base64 -w0
```

Release workflow secrets and variables (documented, not committed):

| Name | Kind | Use |
|---|---|---|
| `ASTROLITH_UPDATE_SIGNING_KEY` | secret | Base64 PKCS#8 Ed25519 private key, or its 32-byte seed. When absent the workflow skips signing and publishes no manifest. |
| `ASTROLITH_UPDATE_MANIFEST_URL` | repository variable or build env | HTTPS URL baked into the release build. |
| `ASTROLITH_UPDATE_PUBLIC_KEY` | repository variable or build env | Base64 raw Ed25519 public key baked into the release build. |
| `ASTROLITH_RELEASE_PAGE` | repository variable | HTTPS release page used in the generated manifest. |

Test keys are generated inside the Rust tests and are never persisted.

## Known gaps

- The icons are provisional assets and have not had a design review.
- No signing key exists in this repository, and no real signed manifest has been
  published or checked by a packaged build.
- `.deb` assembly is exercised locally; AppImage and RPM builds require
  `linuxdeploy`/`appimagetool`/`rpmbuild` and have only been scripted for CI.
- No clean-environment install, launch, file-association, protocol or uninstall
  run has happened for any artifact.
- The repository declares no license. The RPM spec uses `License: UNLICENSED`
  and the AppStream file omits `project_license` until a license is chosen.
- AppStream requires a homepage URL, so the metadata currently points at the
  reserved `https://astrolith.invalid/` placeholder; replace it when a public
  project URL exists.
