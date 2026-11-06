# Rope snapshots and background Writing parsing — 2026-11-06

## Changes

`DocumentBuffer` retains original and current text as copy-on-write Rope snapshots.
A splice records only removed and inserted bytes in source history. Cloning a
buffer or handing a snapshot to a worker does not flatten the document. Existing
`&str` consumers use lazy flat caches; saving, formatting, source-mode edits and
legacy parser boundaries can still materialize contiguous text.

Writing retains the previously absorbed input Rope, compares chunks to identify
one conservative replacement, and maps that replacement into authoritative
source. Normal typing reuses the existing projection String allocation and
updates footer counts from the replaced bytes plus one character on each side.
Hidden imports/frontmatter, UTF-8 byte offsets, CRLF, source undo and save baselines
remain intact. The projection is still a String: moving its tail, comparing
unchanged chunks and exceptional full-projection rebuilds remain linear work.
This change does not claim a fully incremental document engine.

`WriterSurface::render` no longer flattens the editor Rope or parses Markdown.
Session changes supply immutable snapshots outside rendering. One background
parse may run, and one pending slot retains only the latest snapshot. A new
revision cancels obsolete work; the parser checks cancellation between blocks.
A single large block is not interrupted midway. Only the requested revision may
install its result. Dropping the surface cancels its current parse.

While parsing, the previous rich snapshot remains visible. Source coordinates
for unchanged text map into the live input snapshot. Pointer targets inside a
replaced range are rejected without moving the live selection to EOF. Stale link,
image and toolbar actions wait for fresh geometry. Native input continues through
the live EditorState; preedit snapshots also request parsing. Rich text can lag
behind input until the latest parse completes. Parsing is still whole-document
work, now outside the UI thread; it is not an incremental Markdown parser.

No vendor or upstream GPUI patch was added by these changes. At that measurement point, vendor patches from the earlier repair were still
active. The subsequent [unvendoring change](published-gpui-dependencies.md) removes
them. These historical runs do not establish performance for the current
published dependency graph.

## Measured remaining input cost

After removing full-source copying, a temporary stage probe on a 1.2 MB Unicode
headless edit measured 17.60 ms in the session Change handler: 1.41 ms through
source absorption and roughly 16.17 ms in `refresh_writer`. The remaining full
footer token scan caused that delay. Local count updates removed the scan from
normal Writing splices. Exhaustive edit-boundary tests compare local summaries
with full scans across Unicode, apostrophes, underscores, CRLF and empty text.

## Native development-profile verification

Ran `mise run dev` on Hyprland 0.57.1/Wayland with isolated fixture, preferences,
recovery and logs. The fixture uses an Astro discovery stub, not a running Astro
server. Temporary probes exercise the actual native Writer input adapter, shell
mode commands and Hyprland fullscreen/tiled dispatchers. They are removed from
production sources after measurement.

For the 1,112,000-byte multi-block fixture:

- Eight input operations appended `中🦀`, twenty milliseconds apart.
- Session Change handler: 0.827–2.066 ms, including source absorption and counts.
- Initial rich parse: 113.522 ms on Worker-19. Latest revision 8: 107.374 ms on
  Worker-12; intermediate revisions did not install full rich snapshots.
- Largest recorded Writing layout-through-paint CPU sample: 30.151 ms.
- First Build layout-through-paint: 65.035 ms.
- All six directed mode handlers: 0.433–1.858 ms.
- Fourteen fullscreen/tiled actions exercised native resize during mode cycling.

For the 102,420-byte styled CJK/emoji single-paragraph fixture:

- Eight native adapter input operations: session Change handler 0.174–0.266 ms.
- Initial/latest rich parses: 61.189/55.883 ms on background workers.
- Largest Writing layout-through-paint sample: 92.447 ms; several later samples
  were 45–69 ms. First Build sample: 351.434 ms.
- Twelve fullscreen/tiled actions covered the measured mode cycle.

The long paragraph still produces slow frames. This patch removes synchronous
parse and source-copy paths; it does not resolve all shaping/layout work.

Frame probes record CPU request-layout through paint only above 8 ms. They do
not measure GPU presentation, compositor latency or end-to-end keyboard latency.
Programmatic native input-adapter calls do not establish physical keyboard or
fcitx5 candidate-window behavior. Long styled single-paragraph cold shaping is a
separate remaining cost; moving Markdown parsing does not remove it.

Raw native logs, resize timestamps and automated
check output are retained locally under
`/home/maredevi/Documents/Codex/2026-11-06/astrolith-rope-snapshot/`.

## Regression coverage and checks

`mise run check` passes doctor, formatting, lint, workspace build, headless tests
and actual Astro fixture-loader compatibility. Regression coverage includes
shared immutable source snapshots, lazy flat-cache avoidance during splices,
source undo with hidden MDX bytes, reuse of visible projection storage, exact
incremental counts, canceled parsers, coalesced revisions, stale-coordinate
mapping across Rope chunks and IME preedit/commit through the input adapter.

An initial check encountered a save-test race: it waited for filesystem contents
before asserting that the UI completion callback had cleared dirty state. The
isolated retry and subsequent full check passed. No save behavior was changed.
