# M01 shell verification

Date: 2026-10-28. Branch: `codex/m01-shell`.

## Authority and delivered scope

The user accepted all M00 manual checks and authorized the existing M01 plan despite the missing original design specification. [The authority record](m01-authority.md) preserves that decision. Historical spike and plan-review documents remain available.

- M01.1: application-owned semantic light/dark colors; System resolves every native appearance variant and observes window appearance changes. Kit's synchronized theme update path keeps Component and Base projections consistent.
- M01.2: retained Files, Document, Inspector, and optional Preview dock regions. Native resize/collapse events update preferences. Side widths and visibility restore from a versioned settings file; restored and live side extents leave room for the document. Window resizing also constrains the bottom preview height. Rendering performs no filesystem operations.
- M01.3: one command registry, searchable Kit Command inside a standard Dialog, platform shortcuts, and shared toolbar dispatch. A native file picker loads bounded readonly UTF-8 source on a background executor. Cancellation preserves the current source. Editing, document saving, collection schemas, Astro discovery, and running preview services are future milestones.

## Automated evidence

`mise run check` passed doctor, rustfmt, Clippy with warnings denied, locked workspace build, and headless tests. Final run: **30 passed, 0 failed, 1 ignored** (12 desktop tests and 18 shell-library tests). The ignored test is the explicit M00 release performance diagnostic; it is not a skipped M01 behavior test. Full session log: `/tmp/astrolith-m01-final-check.log`.

The new tests exercise system-mode resolution and theme-token application; visibility transitions and restored-width normalization; actual disk preference round trips, failed I/O, malformed/future/external settings preservation, and stale-writer rejection; actual native dock controls through headless pointer dispatch; reconstructed window geometry; command filtering using original item coordinates; no matches; focus restoration; shortcuts while the document owns focus; picker cancellation; readonly typing rejection; and unchanged Unicode/CRLF/MDX source bytes.

Initial tests failed for missing theme/layout/store/command/file APIs before implementation. Kit queues change events until the outer update returns. Tests therefore inspect subscriber state and filtered results after that boundary rather than pressing confirmation against stale search results in one artificial event batch.

## Native evidence and limits

Both Light and Dark shells mapped as native Wayland windows (`xwayland=false`) on Hyprland 0.56.2, initially at 943×1144 logical pixels on the fractional-scale internal display. After the review fix, the final Dark smoke mapped at 1263×1384 logical pixels and its screenshot was inspected again. The screenshots show all three main regions, readable CJK text, theme contrast, toolbar, and status region. Only temporary preference files under `/tmp/astrolith-m01-native` were used. The bounded test processes were terminated after capture; this does not establish graceful-close persistence.

Screenshots and launch logs remain at `/tmp/astrolith-m01-native/{dark,light}.png` and the corresponding launch logs for this session. They are not permanent repository artifacts. These screenshots do not prove physical keyboard shortcuts, OS file-picker integration, native focus restoration, theme-change notifications, or native input/presentation latency. M00's user-provided manual acceptance remains separate evidence. macOS/Windows builds and native behavior were not run here.

## Preference guarantees

Preference writes run on background workers, serialize through one store, ignore older revisions after a newer successful write, synchronize temporary file contents, then atomically replace the target. A shutdown observer attempts to persist the latest in-memory preference snapshot. Malformed/unsupported settings disable writes for the session; external changes also prevent replacement. Errors remain visible while the shell stays usable.

This is application preference storage, not the later document save coordinator. It does not claim crash-proof directory durability, cross-process write serialization, or elimination of the external-check-to-replace race. Readonly file navigation never saves document contents.

## Next milestone

M02 owns Astro project discovery, logical-entry/source mapping, collection indexing, and editable document sessions. Before implementation, incorporate the existing plan review's source-mapping, fixture, and document-lifecycle corrections. Schema generation and lossless field patches remain prerequisites for later persistent forms.

## Independent review and decisions

A fresh-context reviewer inspected the whole implementation range `b2da33d..9e57178`, the plan, authority record, pinned Kit sources, and verification log. No critical findings or deferred minor findings were reported. One important finding was accepted: expanded native docks could hide the document after a window shrink. The new `shrinking_after_expanded_docks_preserves_document_and_panel_geometry` test failed on the original implementation, then passed with live width/height constraints. The full check passed after the fix. This was one fix pass, without a second review.

Implementation decisions, in order:

1. File opening is a readonly UTF-8 snapshot; M02 owns editable sessions and Astro discovery. If that boundary is wrong, editable opening must bring the dirty-document lifecycle forward.
2. Sequential work used a new feature branch in the existing clean checkout. If isolation assumptions change, a separate worktree is required.
3. Headless dock-event assertions run after GPUI flushes pending effects, while geometry still uses real component input. If native ordering differs, platform interaction tests must replace that assumption.
4. Palette tests flush query effects before Enter, matching separate native events. If rapid native ordering differs, investigate that sequence on the platform.

The reviewer explicitly set aside these later-stage or evidence-limited areas; the implementation retains their boundaries:

5. Readonly snapshots have no unsaved-buffer conflicts. M04.2 owns external-file conflicts; editable M02 must establish its session lifecycle first. If scope expands, conflict-safe saves become a prerequisite.
6. Astro unions and nullable objects belong to M03 schema editing. If forms arrive earlier, unsupported schemas must be rejected before editing.
7. M00 IME acceptance is user-provided; editable refresh belongs to M02.4. If editability changes, native IME refresh tests are required.
8. Preview startup and occupied ports belong to M05.1; this shell starts no server. If server startup moves earlier, lifecycle and port-failure handling must move with it.
9. External browser preview remains the Wayland baseline for M05.3/M06.1; embedded preview stays optional. If that baseline changes, native feasibility must be re-established.
10. M01 native interaction and graceful restart acceptance remain unverified. Headless tests and screenshots establish only the stated behaviors. If stronger platform guarantees are required, perform dedicated native interaction and close/reopen acceptance before relying on them.
