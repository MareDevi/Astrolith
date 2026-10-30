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

## M04.3 recovery (implemented)

`crates/astrolith-document/src/recovery.rs` adds a directory-backed recovery store for unsaved content:

- **Records:** `RecoveryRecord` captures identity (project + generation + canonical source), revision, unsaved content and a modified timestamp; `RecoveryRecord::capture(&buffer, modified)` builds one. It deliberately stores **no absolute path**; `resolve_path(project_root)` joins the validated relative `SourcePath`, so a tampered record cannot redirect a write elsewhere.
- **Atomic, keyed persistence:** `RecoveryStore::save` writes JSON atomically (reusing the save coordinator's temp+rename) to a stable file keyed by an FNV-1a hash of project + canonical source (generation-independent, so a record survives reopening), with in-process per-document write serialization.
- **Tolerant load:** `load` returns valid records newest-first plus diagnostics for corrupt/unreadable files; one bad file never aborts recovery. `has_original(project_root)` reports whether the source still exists so a missing original is still recoverable.
- **Correct cleanup:** `clear_after_save(identity, saved_content)` removes a record only when its persisted content equals the just-saved content. Comparing durable content (not the per-buffer revision counter, which resets on reopen) keeps *newer* unsaved content across restarts; `clear` removes unconditionally; an unparseable record is removed on cleanup.

Ten new tests: save/load round-trip; invalid-record diagnostic; valid-JSON-missing-field / bad-identity diagnostics; missing-original still recoverable with path resolved from the project root; obsolete record removed by `clear_after_save`; newer unsaved content kept; corrupt record removed on cleanup; two-document isolation; absent-record clear; directory-named-record diagnostic. Full `mise run check`: **PASS** — **201 Rust tests passed, 0 failed, 4 intentionally ignored**; Astro fixture compatibility PASS.

Independent review of M04.3 found the load path panic-free/tolerant and atomic writes sound, but two [P1]s (both fixed):

- **[P1] Trusted absolute path on load** could let a tampered record direct a write to an arbitrary file. The absolute path is no longer persisted; recovery resolves the validated relative source against a caller-supplied project root.
- **[P1] Revision-based cleanup was unsound across reopening** (the in-memory revision counter resets, so newer content could be kept forever or an older record deleted). Cleanup now compares durable **content**, preserving newer unsaved edits and removing only obsolete ones.
- **[P2]** Added per-document write serialization, plus tests for two-document isolation, corrupt-record cleanup, directory-named records, missing-field/bad-identity diagnostics and path resolution.

## Shell integration: save + recovery (implemented)

Wires the M04 primitives into the GPUI shell (`astrolith-gpui`):

- **Save command** (Ctrl/Cmd+S, palette, Save button): syncs the latest editor text into the buffer, snapshots it, and calls `save_snapshot` off the UI thread. On `Saved` it `confirm_saved`s (clears dirty only if the saved revision is current) and clears the recovery record; on `Conflict` it marks the buffer externally changed, keeps edits, and reports a conflict; I/O errors surface in the status line. Each save/recover has its own UI task, so one cannot drop another's completion.
- **Recovery lifecycle**: dirty edits are captured after a 500 ms debounce into a `RecoveryStore` under the app recovery directory (`ASTROLITH_RECOVERY_DIR` overrides). Clean, saved, and discarded sessions clear their record (per-tab and window-close discard). At project open, pending records matching the current project identity and an indexed source are surfaced.
- **Recover command** (`Recover unsaved changes…`): restores matching records as dirty sessions using the record's own identity (so a later save clears the same record) and the current on-disk file as baseline; disk reads run off the UI thread.

Verification: four new headless GPUI tests — save writes disk and clears dirty; a save conflict preserves disk and keeps edits; a dirty edit records recovery and it is cleared after save; `recover_pending` restores a pending record as a dirty session with the on-disk baseline. GPUI suite: 34 passed. Full `mise run check`: **PASS** — **205 Rust tests passed, 0 failed, 4 intentionally ignored**; Astro fixture compatibility PASS.

Independent review found no P0 and no UI-thread blocking; two P1s fixed: `recover_pending` reused `save_task` (now a dedicated `restore_task`, so a recover no longer drops an in-flight save's completion), and restored sessions discarded the stored identity (now preserved, so cleanup matches and the record is cleared). Pending recovery is additionally filtered by the current project id, and window-close discard now clears recovery. Residual: project identity is launch-local (`workspace-N`), so cross-launch records for a different project with an identical relative source and generation are not fully disambiguated (restore still resolves under the current root, so it cannot write outside the project); records whose original file is missing are skipped at restore.

## Limits and next

- Only scalar leaf patches are supported. Flow collections, block scalars and null leaves are preserved verbatim and read/refused as appropriate; editing them needs a future whole-region editor. This is deliberate, not silent loss.
- A patched float is re-rendered canonically (e.g. `1.50` may become `1.5`); only that field changes.
- Save and recovery are now wired into the shell (Save command, debounced recovery capture, Recover command). Remaining lifecycle polish: auto-apply/prompt for recovery on startup, clearing recovery on window-discard, and native-exit recovery flush; these are shell policy refinements, not document-core gaps. The writer lock is in-process (one application instance).
- The M04 gate ("no known path silently destroys unsaved content or overwrites conflicting disk changes") now has its primitives (lossless patch, conflict-aware save, recovery records); the shell must still adopt them before forms (M03.3) can persist.
