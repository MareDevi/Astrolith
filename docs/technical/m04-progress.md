# M04 — persistence and data safety progress

Started: 2026-10-31. Base: `dev` at `c985ca1` (branch `feat/m04-document`). Scope: **M04.1 lossless document model and field-patch contract only**. Saving, conflict resolution and recovery (M04.2/M04.3) are not started. The original product specification remains absent; this note follows the implementation plan and its review and does not reconstruct that specification or approve later milestone gates.

## Why M04.1 comes before forms (review R3)

The plan-review finding R3 requires a lossless document model and field-patch contract before editable frontmatter forms (M03.3), and requires that untouched source regions are preserved during supported field edits with explicit refusal of unsupported patches. M04.1 delivers exactly that contract. Forms remain non-persistent and blocked until the M04.2 save coordinator passes its gate.

## Implemented contract (M04.1)

`crates/astrolith-document/src/{parse.rs,serialize.rs}` add a pure, filesystem- and GUI-free model:

- **Exact split:** `parse_document` separates the optional frontmatter block (`---` … `---` at the start of the file) from the raw body and keeps the whole original source. `serialize_document` of an unpatched document returns the original bytes, so **round-trip equality is exact by construction** — comments, unknown keys, quoting, Unicode, CRLF/LF and trailing newlines are never re-emitted from a lossy tree.
- **Editable field spans:** for the supported subset — a YAML block mapping of plain keys with plain or quoted scalar values, including nested block mappings — the parser records each scalar's byte span and typed value (`String`, `Integer`, `Float`, `Boolean`, `Null`). `set_field(path, value)` records an in-place replacement of that span and `serialize` splices only those spans. Nested paths use dotted keys (e.g. `author.name`).
- **Preserved-but-opaque:** flow collections (`[...]`, `{...}`) and block scalars (`|`, `>`) are readable but not patchable; a block scalar's whole region is recorded so its body lines are never mistaken for keys.
- **Explicit refusal (no destructive fallback):** anchors/aliases/tags, sequence items, quoted/complex keys, keys with whitespace, tab indentation, duplicate keys and malformed YAML are refused with `DocumentError::{InvalidFrontmatter, UnsupportedFrontmatter}` rather than guessed. YAML validity is additionally checked with `yaml-rust2` before spans are trusted.
- **Safe patch serialization:** strings are emitted plain only when they cannot be reinterpreted (not bool/number/null, no leading indicator, no `: ` or ` #`, no control characters); otherwise single-quoted with `''` escaping. Overlapping patches are refused. `Set_field` on an unknown path is `UnknownField`; on an opaque field is `UnsupportedPatch`.

`DocumentError` gained `InvalidFrontmatter`, `UnsupportedFrontmatter { reason }`, `UnknownField { path }` and `UnsupportedPatch { path, reason }` variants and no longer derives `Copy` (it carries reasons). Existing buffer behaviour is unchanged.

## Verification

Twenty-two new unit tests plus the seven retained buffer tests (29 total in the crate) cover: empty file; file without frontmatter; exact rich round-trip (comments, unknown keys, quoted values, nested mapping, block scalar, flow field, Unicode, CRLF, BOM, no trailing newline); typed/nested value reads; nested and two-level scalar patches preserving every other byte; string patches that require quoting and re-parse; strings that look like booleans/numbers; boolean/integer patches; malformed/unterminated frontmatter; refused anchors/aliases; duplicate keys; unknown-field patch error; block-scalar and flow fields not patchable; null leaves readable but not editable; closing-delimiter trailing whitespace; control-character refusal; trailing-newline preservation.

`mise run check` result: **PASS** — doctor, formatting, warnings-denied Clippy, locked build, workspace tests and the frozen Astro fixture task all succeeded (**152 Rust tests passed, 0 failed, 3 intentionally ignored**; 22 of these are the new M04.1 tests) and **Astro fixture compatibility: PASS**.

## Independent review and resolutions

An independent review verified the lossless core and refuse-not-guess posture and found the R3 contract met, with one [P1] and several [P2]s; all addressed:

- **[P1] Plain-scalar safety used only the crate's own parser.** A value like `"Chapter 1:"` was emitted plain and produced `title: Chapter 1:`, which no longer parses. Plain rendering is now validated by re-parsing `k: <value>` with `yaml-rust2` and requiring an exact string match; otherwise the value is single-quoted. This also fixes cross-tool cases such as `0x1F`, `.inf` and `.nan`.
- **Control characters** are now rejected for patched strings (previously only `\n\r\t` were screened, contradicting the claimed contract).
- **Double-quote escapes:** `\x`, `\b`, `\f`, `\/`, `\a`, `\v`, `\e`, `\U`, `\N`, `\_`, `\L`, `\P` are now decoded so valid files can be opened instead of rejected.
- **Null leaves** (`key:`) are now readable as `FrontmatterValue::Null` (not `UnknownField`) while remaining non-editable; distinguishing a null leaf from a block parent uses the next significant line's indentation.
- **Opaque fields** (flow collections, block scalars) are no longer exposed as a misleading `Some("")`; they read as `None` but still return `UnsupportedPatch` on `set_field`.
- **Delimiters** now tolerate trailing spaces (`---   `).

## M04.2 save coordinator (implemented)

`crates/astrolith-document/src/{save.rs,version.rs}` add the conflict-aware persistence layer:

- **Immutable snapshots:** `DocumentSnapshot` captures path, identity, revision, content and a `DiskBaseline` (the exact bytes at load). `DocumentSnapshot::capture(&buffer, path)` builds one.
- **Conflict check + atomic write:** `save_snapshot` reads the file and writes only if it still equals the baseline; a changed or deleted file yields `SaveOutcome::Conflict` and is not modified. Writes go to a `create_new` temp file in the same directory, `sync_all`, preserve permissions, then `rename`; the parent directory is fsynced best-effort.
- **One writer per document:** saves for the same path are serialized process-wide with a keyed `tokio::sync::Mutex` whose key is the canonicalized path (so relative/absolute spellings share a lock); the actual I/O runs in `spawn_blocking` so no UI thread blocks.
- **Clean only when truly saved:** `DocumentBuffer::confirm_saved(saved_revision, persisted)` clears the dirty baseline only when the saved revision is still current and the content matches; saving revision N while editing N+1 leaves N+1 dirty.
- **Residual race documented:** read-compare-then-rename is not race-free against a third party writing in the check-to-replace window; atomic replacement prevents partial files, not that window.
- **Symlink safety:** renaming the temp file over a symlinked path replaces the link and leaves its target untouched (verified), so saving never follows a link out of the project.

Eleven new save tests cover: clean save and revision; external-change conflict; deleted-file conflict; read I/O error; concurrent single-writer (same path and relative/absolute aliases) with exactly one winner and one conflict; `confirm_saved` keeping newer edits dirty; a stale save persisting revision N while the buffer stays dirty at N+1; symlink replacement; permission preservation; and a failed write leaving the original intact with no temp file. Full `mise run check`: **PASS** — **191 Rust tests passed, 0 failed, 4 intentionally ignored**; Astro fixture compatibility PASS.

Independent review of M04.2 found the conflict core sound (no P0) and raised three [P1]s plus [P2]s; all addressed:

- **[P1] Unix-only `symlink` import in the test module** would break Windows `cargo test`/`clippy`. The import and the symlink/permission tests are now `#[cfg(unix)]`.
- **[P1] The writer lock keyed on the raw path**, so `./a` vs `a`, relative vs absolute, or symlink vs real could interleave. The lock key is now the canonicalized path (falling back to canonicalized parent for a not-yet-existing file); an alias test asserts exactly one writer wins.
- **[P1] A failed `rename` leaked the temp file.** The temp is now removed on rename error, and a read-only-directory test verifies the original is intact with no leftover temp.
- **[P2]** Permissions are set before `sync_all`; `DocumentSnapshot::new` is crate-private (callers use `capture`, which always uses the true load baseline); the residual check-to-replace race and single-writer assumptions are stated precisely in the module docs.

## Limits and next

- Only scalar leaf patches are supported. Flow collections, block scalars and null leaves are preserved verbatim and read/refused as appropriate; editing them needs a future whole-region editor. This is deliberate, not silent loss.
- A patched float is re-rendered canonically (e.g. `1.50` may become `1.5`); only that field changes.
- The broader dirty-document lifecycle coordinator (unsaved tab closure, project switching, application exit, watcher-driven reload decisions) spans the shell and M04.3 recovery; M04.2 provides the save primitive but not those policy decisions. The writer lock is in-process (one application instance).
- M04.3 (recovery of unsaved content) remains. Forms remain blocked until the M04 gate ("no known path silently destroys unsaved content or overwrites conflicting disk changes") is judged closed, which needs M04.3 as well.
