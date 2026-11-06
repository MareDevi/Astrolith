# Dependency refresh — 2026-11-06

Queried crates.io for every application Rust dependency, then updated direct
requirements and refreshed the complete Cargo.lock graph. GPUI remains unpatched.
Frozen Astro compatibility fixtures and project tool versions are separate test
inputs and were not changed by this Rust dependency update.

| Direct dependency | Before | After |
| --- | --- | --- |
| gpui-kit / optional gpui-webview | 0.8.1 | 0.9.0 |
| base64 | 0.22.1 | 0.23.1 |
| trash | 5.2.9 | 5.3.0 |
| nix | 0.29.0 | 0.31.3 |
| windows-sys | 0.52.0 | 0.61.2 |
| yaml-rust2 | 0.11.1 | 0.13.0 |

GPUI Kit resolves gpui-base/component 0.9.0 and gpui-pre 0.3.11. Other direct
requirements already matched their latest stable releases. Ropey remains on the
existing, latest 2.0.0-beta.1 API line instead of downgrading to stable 1.6.1.
notify remains stable 8.2.1 instead of opting into 9.0.0-rc.7.

One upgrade is constrained: tree-sitter 0.27.1 cannot coexist with the 0.26.13
native library required by GPUI Kit 0.9.0. Cargo rejects two crates with the same
`links = "tree-sitter"` value. Astrolith therefore retains exact 0.26.13 for its
AST services; resolving the conflict would require an upstream-compatible Kit
release or replacing that part of the dependency graph.

Windows job ownership now uses standard OwnedHandle, including null checks and
borrowed raw handles at FFI boundaries. It closes once on drop and preserves
Send/Sync when moved through worker tasks. Removed the obsolete Child
AsRawHandle shim; Tokio exposes Child::raw_handle directly.

Validation: `mise run check` passed, including 1,011 Rust tests (8 ignored) and
actual Astro fixture-loader compatibility. Optional embedded-webview cargo check
passed. An isolated Windows-target compilation checked the actual job wrapper
against windows-sys 0.61.2 and asserted Send + Sync; this is not a complete Windows
application build or native Windows runtime test.

Version inventory, Cargo resolution logs and check output are retained with the
native profiling evidence produced after this upgrade.

See [the post-upgrade native profile](editor-bottlenecks-0.9.md) for measured
rendering hotspots and the Editor/EditorState/TextView ownership distinction.
