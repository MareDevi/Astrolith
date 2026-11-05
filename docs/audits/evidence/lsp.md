# LSP client and editor integration

## Scope inspected

| File | Lines | Read |
|---|---|---|
| `crates/astrolith-lsp/src/lib.rs` | 28 | full |
| `crates/astrolith-lsp/src/client.rs` | 1145 | full (events, commands, actor, handshake, serve loop, server-message dispatch, param builders, tests 904-1145) |
| `crates/astrolith-lsp/src/framing.rs` | 229 | full |
| `crates/astrolith-lsp/src/discovery.rs` | 291 | full |
| `crates/astrolith-lsp/src/language.rs` | 193 | full |
| `crates/astrolith-lsp/src/position.rs` | 274 | full |
| `crates/astrolith-gpui/src/editor/lsp.rs` | 317 | full |
| `crates/astrolith-gpui/src/editor/session.rs` | 2081 | LSP regions 200-253, 1400-1683, 1740-1803, plus greps for every `lsp*` field and caller |
| `crates/astrolith-gpui/src/project_discovery.rs` | — | 3590-3680 (session creation/activation), 3835-3850 (close), 2655-2700 (definition navigation) |
| `crates/astrolith-gpui/src/runtime.rs` | 90 | full (tokio runtime shape) |
| `gpui-base-0.8.1/src/input/editor/lsp/` | — | `mod.rs` (Lsp struct, `apply_lsp_edits`), `completions.rs` 146-300 + 366-410, `diagnostics.rs` 18-74, provider trait list |
| `gpui-pre-scheduler-0.3.10/src/executor.rs` | — | 394-420 (`Task` drop = cancel) |
| Docs | — | `docs/technical/editor-capabilities.md`, `docs/refactor/ARCHITECTURE.md` (21, 23, 122, 128), `docs/technical/concurrency-and-responsiveness.md` |

Not inspected: no language server was executed; no build/test/benchmark run (read-only task, build cache in use). Requirement 5 of the brief — "bytes copied per keystroke" — is answered from data flow, never from a measurement. Every performance statement is marked with its evidence level; none is Confirmed-by-measurement.

Shape of the system: `Client` owns one `Connection` (writer + `pending` map) driven by one actor task; `Client::handle()` clones a `ClientHandle` (command sender + shared version counter) into up to four editor providers; `DocumentSession` owns the `Client` and pumps `Event`s on a GPUI task.

## Findings

### Q1 — Server lifecycle: ownership, root, count, restart, shutdown

**Lifecycle state machine as implemented** (`client.rs`, `session.rs`, `project_discovery.rs`):

```mermaid
stateDiagram-v2
    [*] --> Idle: "DocumentSession::new<br/>lsp = None (session.rs:380-384)"
    Idle --> Discovering: "set_source_view(true)<br/>+ project_root + source_view<br/>(session.rs:1480-1493)"
    Idle --> NoServer: "LanguageId::server() == None<br/>Markdown (language.rs:129)"
    Discovering --> Handshaking: "discover() ok<br/>spawn_process (client.rs:247-291)"
    Discovering --> Unavailable: "DiscoveryError / spawn error<br/>lsp_unavailable = true<br/>(session.rs:1512-1521)"
    Handshaking --> Ready: "initialize result<br/>Event::Ready (client.rs:462-464)"
    Handshaking --> Failed: "Event::Failed<br/>child killed after 300 ms<br/>(client.rs:466-490)"
    Ready --> Ready: "didOpen / didChange debounce 150 ms<br/>provider requests"
    Ready --> Exited: "stdout EOF or framing error<br/>(client.rs:485-489)"
    Failed --> Exited
    Ready --> Killed: "stop_lsp -> drop(Client) -> abort actor<br/>kill_on_drop (client.rs:255, 339-345)"
    Exited --> Zombie: "logs only (session.rs:1595-1600)<br/>lsp stays Some, handle closed"
    Zombie --> Killed: "tab / mode switch"
    Unavailable --> Discovering: "stop_lsp / set_project_root<br/>reset the latch"
    Killed --> Discovering: "re-enter Build (session.rs:1613-1616)"
    Killed --> [*]: "session dropped<br/>(project_discovery.rs:3844)"
    NoServer --> [*]
    Unavailable --> [*]
```

#### LSP-01 — One server process per open Build document; no per-language or per-workspace reuse
- **Evidence**: `session.rs:1545-1550` constructs `LspState { _client: client, .. }` per document inside `attach_lsp`; no registry exists (`client.rs:359` is the only `HashMap`, the pending-request map). `language.rs:23-25` documents `ServerDef.id` as "Stable server identity for logs and one-server-per-language reuse" — that reuse has no implementation.
- **Level**: Confirmed (source).
- **Impact**: five open `.ts`/`.astro` tabs = five `typescript-language-server`/`astro-ls` processes, five `initialize` project scans, five id sets. Switching tabs tears a server down and rebuilds it (`project_discovery.rs:3664-3670`), so tab-hopping pays full startup each time.

#### LSP-02 — Graceful shutdown path is unreachable production code; processes are SIGKILLed
- **Evidence**: `ClientHandle::shutdown` (`client.rs:232-236`) is invoked only by the unit test at `client.rs:1031`. `stop_lsp` sends `didClose` then immediately `drop(state)` (`session.rs:1620-1622`), which aborts the actor (`client.rs:339-345`) while `kill_on_drop(true)` (`client.rs:255-256`) owns the process. The `shutdown` request → `exit` notification → 2 s grace sequence lives at `client.rs:746` and `client.rs:474-479` but only runs if the actor survives to process `Command::Shutdown`.
- **Level**: Confirmed (source); the didClose-vs-abort race means `didClose` is usually never written either.
- **Impact**: protocol violation (`shutdown`/`exit` are required before termination); servers that flush caches, write `.astro`/tsbuildinfo state, or clean temp dirs on `exit` cannot. Crash-only teardown also hides protocol bugs from logs.

#### LSP-03 — No restart, no recovery, no user-visible failure state after crash or failed handshake
- **Evidence**: `LspEvent::Failed`/`Exited` are only logged (`session.rs:1595-1600`); `self.lsp` stays `Some`, the actor is gone, and `ClientHandle::open/change/close` discard the send error (`client.rs:191-208`), as does `request` (`client.rs:211-229`). Restart requires a mode switch or tab switch (`stop_lsp`).
- **Level**: Confirmed (source).
- **Impact**: after a server crash (OOM, `node` exit, dependency install mid-session) the document silently loses completion/hover/definition/diagnostics with no indicator; typing produces no diagnostics and no error surface beyond a `tracing::info!` line.

#### LSP-04 — Discovery-failure latch is per session and only resets on stop/root change
- **Evidence**: `lsp_unavailable` (`session.rs:110`) short-circuits `start_lsp` (`session.rs:1481`), set at `session.rs:1519`, cleared in `stop_lsp` (`session.rs:1614`) and `set_project_root` (`session.rs:1476`).
- **Level**: Confirmed.
- **Impact**: matches the documented intent (retry after a tab/mode round-trip), but a document left open in Build never retries even if the server appears on disk — no filesystem watch, no retry timer.

#### LSP-05 — Document URI is built from an uncanonicalized path
- **Evidence**: `path = root.join(self.canonical_source())` (`session.rs:1487`) → `uri_from_path` (`client.rs:153-160`), which only requires an absolute path. `discover` canonicalizes the *root* (`discovery.rs:71-76`) but nothing canonicalizes the document. Diagnostics `uri` is ignored by the consumer (LSP-26), hiding any mismatch.
- **Level**: Hypothesis (needs a real server; the code path is certain, the resulting URI string divergence is not).
- **Impact**: symlinked workspaces or `..` segments can make the client's `file://` URI differ from the server's normalized one; publish/listener matching then silently fails, and only the uri-ignoring consumer keeps diagnostics working by accident.

#### LSP-06 — Startup ordering is correct (no didOpen/request before `initialized`)
- **Evidence**: `install()` + `state.open()` run immediately after `spawn_process` (`session.rs:1551-1552`), but the actor's `serve` loop — the only consumer of `commands` — starts after `handshake` returns (`client.rs:458-471`), so `didOpen` and provider requests queue harmlessly.
- **Level**: Confirmed.
- **Impact**: none; recorded because the code reads like a race and a future refactor could break it.

### Q2 — Protocol correctness: framing, ids, routing, cancellation, timeouts, server requests

#### LSP-07 — No request timeout anywhere; a wedged server hangs provider tasks forever
- **Evidence**: `ClientHandle::request` awaits a `oneshot` with no deadline (`client.rs:211-229`); the crate's only `tokio::time::timeout` bounds child exit (`client.rs:479`). Providers await it inside `background_spawn` (`lsp.rs:137-149`, `174-187`, `202-224`, `250-256`).
- **Level**: Confirmed (source); user-visible frequency is Hypothesis.
- **Impact**: hover popover never appears, completion never resolves, definition never navigates — with no error, no spinner timeout, and no way for the user to retry other than moving the caret (which spawns *another* hanging request). Each abandoned request stays in `pending` until the server answers.

#### LSP-08 — `$/cancelRequest` is never sent
- **Evidence**: `grep -r cancelRequest crates/ apps/` → no match. Superseded requests are dropped only on the consumer side: kit resets `context_menu_task = Task::ready(Ok(()))` and rejects results by document revision/cursor (`gpui-base .../lsp/completions.rs:377, 391-406`).
- **Level**: Confirmed.
- **Impact**: pure server-side waste, but it is the mechanism that turns LSP-21 (one completion per keystroke) into a growing queue inside tsserver/astro-ls; in-order server queues mean the useful request is answered last.

#### LSP-09 — `pending` grows monotonically for unanswered ids
- **Evidence**: `next_id` increments forever (`client.rs:386-390`); entries are removed only by `resolve` (`client.rs:426-444`).
- **Level**: Confirmed.
- **Impact**: with LSP-07/08, every abandoned request leaks a map slot plus a `oneshot::Sender` for the lifetime of the server process.

#### LSP-10 — Non-integer response ids are silently discarded
- **Evidence**: `message.get("id").and_then(Value::as_i64)` (`client.rs:769-771`) returns early for a string id, with no log. Unknown numeric ids log at debug (`client.rs:438-441`).
- **Level**: Confirmed (code); reachability requires a non-conforming server (Hypothesis).
- **Impact**: would surface as a timeout (LSP-07) rather than a diagnosable error.

#### LSP-11 — Correlation is sound; document versioning is absent from the request path
- **Evidence**: exact `HashMap<i64, Pending>` lookup (`client.rs:426-444`) handles out-of-order responses; `Command::Request` carries no version (`client.rs:86-90`), so the client itself cannot tell a stale answer from a fresh one. Kit's revision/cursor gate (`completions.rs:402-406`) covers completion; hover/definition rely on position validity.
- **Level**: Confirmed.
- **Impact**: no wrong-data leak observed for completion; hover/definition can still act on a pre-edit answer if the offset is still in range.

#### LSP-12 — Server-initiated requests are answered with placeholders and the client ignores every result
- **Evidence**:

| Server request | Reply | Line |
|---|---|---|
| `workspace/configuration` | `[null; items.len()]` | `client.rs:821-826` |
| `client/registerCapability`, `client/unregisterCapability`, `window/workDoneProgress/create`, `window/showMessageRequest` | `null` | `client.rs:827-831` |
| `workspace/applyEdit` | `{"applied": false}` | `client.rs:832-833` |
| anything else (`workspace/workspaceFolders`, `window/showDocument`, `workspace/*/refresh`) | error `-32601` | `client.rs:834-835` |
| `$/progress`, `telemetry/event`, `$/logTrace` | dropped | `client.rs:836-837` |

- **Level**: Confirmed.
- **Impact**: (a) dynamic registrations are acknowledged and thrown away, so the client and server disagree about which features exist; (b) every settings-dependent server (vscode-json/css/html, astro-ls tsdk, ts-ls preferences) runs on defaults forever; (c) `applyEdit` is permanently refused, so server-driven refactors cannot land; (d) `window.workDoneProgress: true` is advertised (`client.rs:580-583`) while progress notifications are dropped, so long operations have no UI; (e) `window/showDocument` is rejected as a request even though the kit has a local `show_document` handler (`session.rs:1555-1570`) — the two paths are unrelated.

#### LSP-13 — Framing: correct happy path, lenient edges, two latent desync cases
- **Evidence**: `encode` emits `Content-Length: {byte_len}\r\n\r\n` + body (`framing.rs:44-56`). `header_boundary` accepts CRLFCRLF *or* LFLF (`framing.rs:124-135`); `content_length` is case-insensitive, trims, first match wins (`framing.rs:137-155`); missing header → `Malformed` (`framing.rs:99-105`); EOF mid-body → `Malformed` (`framing.rs:82-87`); 32 MiB cap on announce and on buffer (`framing.rs:12, 89, 107-109`). Defect (a): the `.position(\r\n\r\n)` search spans the whole buffer and is preferred over an earlier `\n\n` boundary, so mixed line endings can split a frame at the wrong offset. Defect (b): duplicate `Content-Length` headers are accepted (first wins) instead of rejected.
- **Level**: Strong for both defects (source-level); Hypothesis for real servers (mainstream servers emit CRLF only).
- **Impact**: a server emitting mixed terminators or duplicate headers would desync the stream and be reported as an unreadable EOF/JSON error rather than a protocol diagnosis.

#### LSP-14 — Each frame is copied three times and drained from the front
- **Evidence**: `body.to_vec()` then `self.buffer.drain(..body_end)` (`framing.rs:115-116`) plus `serde_json::from_slice` (`framing.rs:117-120`); reads are 8 KiB chunks (`framing.rs:77`) so the buffer grows by repeated reallocation.
- **Level**: Confirmed (source).
- **Impact**: for a 32 MiB payload ≈ 96 MiB of memory traffic and one large memmove per message on a shared tokio worker (LSP-37).

#### LSP-15 — Encoding negotiation is one-way
- **Evidence**: the client offers `positionEncodings: ["utf-16"]` (`client.rs:531-534`) but never reads `InitializeResult.capabilities.position_encoding`; `handshake` only deserializes `ServerCapabilities` (`client.rs:629-635`) and the session discards it (LSP-17). All conversions assume UTF-16 (`position.rs:1-8`).
- **Level**: Strong.
- **Impact**: a server that answers `utf-8`/`utf-32` (allowed when it ignores the offered list, or once the advertised set is widened) would have every position mis-converted with no diagnostic.

### Q3 — Capability negotiation: asked, stored, used

#### LSP-16 — The requested capability set is exactly the integrated feature set, and it is narrow
- **Evidence**: `initialize_params` (`client.rs:518-590`) declares text sync with `didSave/willSave/willSaveWaitUntil: false` (`536-541`), completion with `snippetSupport:false`, `insertReplaceSupport:false` (`542-556`), hover (`557-560`), definition with `linkSupport:true` (`561-564`), codeAction with literal kinds + `isPreferredSupport` (`565-573`), publishDiagnostics with version/tag support (`570-575`), `workspace.{workspaceFolders,configuration,applyEdit}` (`576-579`), `window.{workDoneProgress,showMessage,showDocument}` (`580-585`), `initializationOptions: null`, `trace: "off"` (`586-589`). Absent: references, rename, documentSymbol, workspaceSymbol, signatureHelp, formatting/rangeFormatting, semanticTokens, documentColor, inlayHint, foldingRange, selectionRange, callHierarchy, executeCommand, pull `diagnostic`, `workspace.didChangeWatchedFiles`, `workspace.didChangeConfiguration`.
- **Level**: Confirmed.
- **Impact**: servers gate optional features on the client's declared capabilities, so absent declarations are not just unused — they suppress server-side work that would otherwise be answerable. `initializationOptions: null` additionally means astro-ls gets no TypeScript SDK hint.

#### LSP-17 — Capabilities are transported, then dropped; providers are not gated
- **Evidence**: `Event::Ready(Box<ServerCapabilities>)` (`client.rs:107`, sent `client.rs:463`); the session matches `LspEvent::Ready(_) => {}` (`session.rs:1601`). `install()` wires all four providers unconditionally (`lsp.rs:59-62`).
- **Level**: Confirmed.
- **Impact**: requests are sent to servers that never advertised the feature (error `-32601`), the kit converts that into an `anyhow` error it does not display, and there is no place in the codebase where "server supports X" is known — so future features cannot be gated without new plumbing.

#### LSP-18 — Kit supports semantic tokens and document colors; neither is installed or negotiated
- **Evidence**: `gpui-base .../lsp/mod.rs:47-49` (`semantic_tokens_provider`, `document_color_provider`), `semantic_tokens.rs:36`, `document_colors.rs:13`; `lsp.rs:59-62` sets neither, and LSP-16 never declares them.
- **Level**: Confirmed.
- **Impact**: the two cheapest available feature wins are blocked only by a missing provider impl + two capability keys.

### Q4 — Document sync: notifications, versions, bytes per keystroke

#### LSP-19 — Always full-text `didChange`, ignoring the server's declared sync kind
- **Evidence**: `Command::Change` always sends one `contentChanges` entry with the whole document and no range (`client.rs:717-725`); `TextDocumentSyncKind` lives in the discarded `ServerCapabilities` (LSP-17). Copies per settled burst: `buffer.current_text().to_owned()` on the UI thread (`session.rs:1658`), move into `Command` (`client.rs:201-205`), `serde_json::to_vec` in `encode` (`framing.rs:44-48`, includes escaping).
- **Level**: Confirmed (source); cost magnitude per file size is arithmetic, not measured.
- **Impact**: ≈3× document bytes copied per typing pause plus a full server-side reparse; on a 500 KiB `.astro` file that is ~1.5 MiB of copying per pause, with no incremental path even for servers that declared `Incremental`.

#### LSP-20 — The 150 ms debounce is a task-drop cancel, and it copies before it can know it is superseded
- **Evidence**: each edit stores a new `Task` in `lsp_change` (`session.rs:1659-1666`); dropping a gpui `Task` cancels it (`gpui-pre-scheduler-0.3.10/src/executor.rs:396-399`: "If you drop a task it will be cancelled immediately"). The full-text clone happens at `session.rs:1658`, before the timer.
- **Level**: Confirmed.
- **Impact**: correct coalescing, but every keystroke pays one document copy that is thrown away (linear in file size, on the UI thread).

#### LSP-21 — Completion is issued per keystroke, with a UI-thread document copy each time
- **Evidence**: `is_completion_trigger` accepts any alphanumeric plus `_ . $ @ / - " ' <` (`lsp.rs:152-160`); kit routes every non-silent typed character through `on_text_typed` → `handle_completion_trigger` → `request_completions` with no debounce (`gpui-base .../input/editor/mod.rs:98-106`, `.../lsp/completions.rs:228-291`); the provider does `text.to_string()` before `background_spawn` (`lsp.rs:123-129`).
- **Level**: Confirmed (source); latency impact unmeasured.
- **Impact**: one full-document copy on the render thread + one `textDocument/completion` round-trip per typed character, uncancelled (LSP-08). This is the dominant per-keystroke cost in the whole integration, well above the `didChange` traffic that the 150 ms debounce was added to control.

#### LSP-22 — `triggerCharacter` is the whole typed word
- **Evidence**: kit passes `CompletionContext { trigger_kind: TRIGGER_CHARACTER, trigger_character: Some(query) }` where `query` is the entire prefix (`gpui-base .../lsp/completions.rs:283-291`); our provider forwards it verbatim (`lsp.rs:139-142`). The spec requires a single character.
- **Level**: Confirmed.
- **Impact**: servers that switch on specific characters (`.` for members, `<` for tags) cannot; they fall back to invoked-completion behaviour.

#### LSP-23 — No `didSave`; the client explicitly declares it unsupported
- **Evidence**: `didSave: false` (`client.rs:538-540`); no save notification is sent anywhere in `crates/`. Saves go through `save_coordinator` with no LSP hook.
- **Level**: Confirmed.
- **Impact**: save-triggered server work (astro-ls content-collection refresh, tsserver project reload, formatting-on-save negotiation) never happens; the server must infer everything from `didChange`.

#### LSP-24 — One version counter per client, shared by `open` and `change`
- **Evidence**: `version.fetch_add` in both (`client.rs:192, 202`), one `Arc<AtomicI32>` per `Client::start` (`client.rs:307-309`).
- **Level**: Confirmed.
- **Impact**: monotonic per document today (one document per client), but a latent hazard the moment LSP-01 is fixed by reusing a client across documents.

#### LSP-25 — Code-action edits do reach the server; nothing rejects an edit made while a request is in flight
- **Evidence**: kit's `replace_text_in_range_silent` suppresses only `on_text_typed`, not `InputEvent::Change` (`gpui-base .../input/base/state.rs:4709-4718`), so the session absorbs the edit and calls `sync_lsp_change` (`session.rs:234-236`).
- **Level**: Confirmed.
- **Impact**: the sync path is sound; the missing piece is request versioning (LSP-11), so a hover/definition answer computed before the edit can still be applied if its offsets remain valid.

### Q5 — Diagnostics and coordinate handling

Coordinate contract, quoted verbatim (`position.rs:1-8`):

> `//! Position conversions between LSP (UTF-16 code units) and this editor.`
> `//! The GPUI editor's 'Position::character' counts Unicode scalar values while LSP counts UTF-16 code units. Servers send UTF-16 positions; every position crossing into the UI must be converted, or ranges drift after an emoji or another non-BMP character.`

and (`position.rs:190-193`):

> `/// Servers answer in UTF-16; the editor converts a Position::character as a character count. Converting the response once, at the protocol boundary, keeps every provider free of ad-hoc position math.`

- Push only. There is no `textDocument/diagnostic` (pull) support: not declared (LSP-16), no method constant (`client.rs:891-903`), no result-id handling.

#### LSP-26 — `publishDiagnostics` uri and version are parsed and then discarded; every publish is applied to the current editor
- **Evidence**: `Event::Diagnostics { uri, version, diagnostics }` (`client.rs:108-114`) is filled from `PublishDiagnosticsParams` (`client.rs:787-806`, `version` from the wire, `client.rs:790-794`). The session destructures `LspEvent::Diagnostics { diagnostics, .. }` (`session.rs:1577`) and calls `apply_diagnostics(&session.editor, &diagnostics, cx)`; `apply_diagnostics` takes no `uri`/`version` (`lsp.rs:87-91`) and reuses the current editor text for conversion (`lsp.rs:92-101`).
- **Level**: Confirmed (code path); real-server frequency is Hypothesis.
- **Impact**: three concrete failures — (a) a publish for a *different* URI (astro-ls/tsserver publish for project files never opened here) is painted onto this document at converted positions; (b) an in-flight publish for version N overwrites the set published for N+1, so diagnostics can regress while typing; (c) the conversion uses text the publish does not describe, so stale ranges drift or clamp to the line start. Highest-impact defect in the module because it is a silent wrong-data path, not a missing feature.

#### LSP-27 — Position conversion is O(document) per position, in a recursive JSON walk
- **Evidence**: `line_start` scans from byte 0 for every lookup (`position.rs:56-70`); `convert_positions` recurses over every object and array, rewriting any `{start,end}` pair that parses as a `Position` (`position.rs:195-232`).
- **Level**: Confirmed (source); magnitude per payload unmeasured.
- **Impact**: a several-hundred-item completion list with `textEdit.range` performs hundreds of full-document line scans in the background task — CPU on a 2-worker runtime (LSP-37), not UI jank.

#### LSP-28 — Cross-file positions are converted against the wrong document
- **Evidence**: the definition provider converts the whole response with the *current* document (`lsp.rs:206-208`) although `GotoDefinitionResponse` points into other files (`lsp.rs:209-231`); `utf16_position_to_char` then measures the target `character` on a line of the source file (`position.rs:11-15, 96-101`), and `line_slice` returns `""` for a line beyond the source document (`position.rs:72-79`), collapsing the column to 0. `reveal_source_position` converts char→byte against the *target* text (`session.rs:1627-1648`), so the wrong column is preserved.
- **Level**: Strong (code); observable drift requires non-BMP characters on the relevant lines (Hypothesis).
- **Impact**: jump-to-definition into a file whose target line does not exist in the source document always lands at column 0; with non-BMP characters it lands at the wrong column. Silent, and worse than no feature for navigation trust.

#### LSP-29 — Diagnostics are replaced per publish and cleared only on uninstall
- **Evidence**: `*current = set` per publish (`lsp.rs:94-105`); `uninstall` clears (`lsp.rs:76-78`), driven by mode switch / tab switch / close (`session.rs:1612-1623`, `project_discovery.rs:3844`).
- **Level**: Confirmed.
- **Impact**: consistent with the push model; combined with LSP-02 (no `didClose` reaching the server) a server can keep publishing for a document the client considers closed — wasted work, and the events die in a dropped pump, so no leak.

### Q6 — Feature coverage

Legend: **implemented** = end-to-end path exists and is wired; **partial** = path exists with a known defect; **missing** = no code; **defective** = wired but wrong.

| Feature | Status | Evidence | Note |
|---|---|---|---|
| Push diagnostics | partial | `client.rs:787-806`, `lsp.rs:87-108`, `session.rs:1577` | no uri/version filter (LSP-26) |
| Pull diagnostics | missing | not declared `client.rs:536-577`; no method constant | |
| Completion (menu + Ctrl+Space) | implemented | `lsp.rs:119-160`, `client.rs:843-851` | full-doc copy per keystroke (LSP-21) |
| Completion snippets / insert-replace | missing | `client.rs:545-547` (`false`) | plain-text inserts only |
| `completionItem/resolve` | missing | no method constant (`client.rs:891-903`) | docs/details never lazily fetched |
| Hover | implemented | `lsp.rs:164-190`, `client.rs:852-860` | no timeout (LSP-07) |
| Definition (incl. LocationLink) | partial | `lsp.rs:192-233`; navigation `session.rs:1555-1570` → `project_discovery.rs:2665-2691` | cross-file columns wrong (LSP-28) |
| References | missing | no kit trait (only 6 traits exist in `gpui-base .../lsp/`) | |
| Rename / prepareRename | missing | no kit trait, not negotiated | |
| Code actions | partial | `lsp.rs:240-287`, `client.rs:864-877` | resolve/command actions silently no-op (LSP-30) |
| `workspace/executeCommand` | missing | `lsp.rs:267-268` comment; no handler in `client.rs:821-835` | |
| `workspace/applyEdit` (server→client) | missing (refused) | `client.rs:832-833` | |
| Formatting / range formatting | missing | not declared, no trait, no method | |
| Semantic tokens | missing | kit trait exists (`semantic_tokens.rs:36`); not installed (`lsp.rs:59-62`), not negotiated | cheapest win |
| Document colors | missing | kit trait exists (`document_colors.rs:13`); not installed | |
| Workspace symbols | missing | no trait, not negotiated | |
| Document symbols | missing | no trait, not negotiated | |
| Signature help | missing | no trait, not negotiated | |
| Inlay hints | missing | no trait, not negotiated | |
| Folding / selection range | missing | folding is tree-sitter-based (`session.rs:1448`) | |
| File watching (`didChangeWatchedFiles`) | missing | absent from `client.rs:576-586` | external disk changes invisible |
| Diagnostics panel / list navigation | missing | only gutter/underline via `DiagnosticSet` (`lsp.rs:94-105`) | |

#### LSP-30 — Code actions that need `codeAction/resolve` or carry a `Command` do nothing, silently
- **Evidence**: `perform_code_action` returns `Task::ready(Ok(()))` when `action.edit` is `None` (`lsp.rs:283-287`); `Command` variants are filtered out of the offered list (`lsp.rs:264-270`) with the comment "this client has no workspace/executeCommand flow". `code_action_params` sends `"context": {"diagnostics": []}` and no `only` filter (`client.rs:864-877`).
- **Level**: Confirmed.
- **Impact**: organize-imports and most ts-ls/astro-ls quickfixes are either hidden or no-ops; a user sees `Ctrl+.` do nothing with no error. Empty `context.diagnostics` also prevents diagnostics-keyed quickfixes that require the context.

#### LSP-31 — Workspace edits are applied partially, and multi-edit application is order-sensitive
- **Evidence**: `apply_workspace_edit` skips foreign-URI entries and drops `documentChanges` entirely (`lsp.rs:296-317`, log lines 306 and 315); kit's `apply_lsp_edits` loops edits calling `replace_text_in_range_silent` one at a time, recomputing each range against the already-mutated text (`gpui-base .../lsp/mod.rs:143-160`), with no version check against the server's `documentChanges[].textDocument.version`.
- **Level**: Confirmed.
- **Impact**: LSP workspace-edit semantics are "all ranges in the original document"; sequential application corrupts any action with more than one edit unless the server happens to order them descending. Combined with LSP-30, the code-action feature is trustworthy only for single plain edit-in-current-file actions.

#### LSP-39 — Definition navigation opens any canonicalized path the server names
- **Evidence**: the `show_document` hook emits `DefinitionTarget { path, selection }` for any URI != own (`session.rs:1556-1569`); `open_definition_target` canonicalizes and opens it through `open_browser_path` (`project_discovery.rs:2665-2691`) with no project-root containment check at this layer.
- **Level**: Hypothesis (depends on whether `open_browser_path` rejects outside-root paths — `editor-capabilities.md` claims outside files stay read-only snapshots).
- **Impact**: worst case is a read-only tab outside the project; not a code-execution risk, but it means the trust boundary is enforced (or not) in a different module than the one that accepts the URI.

### Q7 — Discovery

#### LSP-32 — Order, candidates and absence behaviour
- **Evidence**: `LanguageId::from_path` maps extensions (`language.rs:33-68`); Markdown resolves to `None` and fails as `Unsupported` before any filesystem access (`language.rs:129`, `discovery.rs:68-70`). Then: canonicalize root (`discovery.rs:71-76`) → walk `root.ancestors()` closest-first joining `node_modules/.bin` (`discovery.rs:78-86`) → `PATH` via `std::env::split_paths` (`discovery.rs:87-95`) → `NotFound { language, binaries }` with the candidate list in the message (`discovery.rs:96-101, 54-59`). Servers: `astro-ls`; `typescript-language-server` (ts/tsx/js/jsx); `vscode-json-language-server` (json/jsonc); `vscode-css-language-server` (css/scss/less); `vscode-html-language-server`; `svelteserver`; `vue-language-server` — all with `--stdio` (`language.rs:90-131`). Windows wraps `.cmd/.bat` in `cmd /C` (`discovery.rs:105-131`); unix requires the `0o111` bit (`discovery.rs:177-190`).
- **Level**: Confirmed.
- **Impact**: matches the documented contract; absence degrades to a log line plus `lsp_unavailable` (`session.rs:1512-1521`) with highlighting retained.

#### LSP-33 — No install/network, but the search climbs to the filesystem root with no project ownership check
- **Evidence**: `for directory in root.ancestors()` (`discovery.rs:78`) walks to `/`; there is no `package.json` dependency check, no version check, and no manifest validation. The crate manifest has no HTTP client (`crates/astrolith-lsp/Cargo.toml`). `is_executable` only tests the mode bit, so a symlink to a non-runnable shim passes and fails at spawn (`client.rs:257-260` → `Event::Failed`).
- **Level**: Confirmed (code); practical mis-resolution is Hypothesis.
- **Impact**: `$HOME/node_modules/.bin/astro-ls` from an unrelated project can be launched for this one, with the wrong version; the failure mode is a health/handshake error rather than "server not found".

#### LSP-34 — Blocking filesystem calls on the shared async workers
- **Evidence**: `canonicalize`, `metadata`, `env::var_os("PATH")` execute inside the `runtime::spawn` async block (`session.rs:1497-1503`, `discovery.rs:71-95, 177-190`), i.e. on one of two tokio workers shared with documents, preview and terminal (`runtime.rs:53-58`); the blocking semaphore is not used.
- **Level**: Confirmed.
- **Impact**: a cold or network-backed workspace root stalls half the service reactor for the duration of the stat storm; small in absolute terms but it is the only service that bypasses the blocking pool.

### Q8 — Concurrency and reader-loop behaviour

#### LSP-35 — Three unbounded queues; no backpressure from protocol to UI
- **Evidence**: command channel `client.rs:305`, event channel `client.rs:306`, incoming channel `client.rs:457`, all `mpsc::unbounded_channel`. The actor processes one message at a time and awaits `write_all` + `flush` per server request reply (`client.rs:413-424`); the UI pump rebuilds a `DiagnosticSet` per publish (`session.rs:1575-1603` → `lsp.rs:88-105`).
- **Level**: Confirmed.
- **Impact**: a server flooding `$/progress` or re-publishing diagnostics during a project-wide check grows `incoming` and `events` without limit while the UI is behind; memory grows with server verbosity, never with user action.

#### LSP-36 — JSON parse/serialize are off the UI thread; document copies and diagnostic projection are on it
- **Evidence**: off-UI — framing read + `from_slice` (`framing.rs:117-120` via `read_loop`, `client.rs:494-515`), diagnostics `from_value` (`client.rs:788`), request serialization (`framing.rs:44-48`), provider bodies in `background_spawn`. On-UI — `buffer.current_text().to_owned()` per `didChange` (`session.rs:1658`), `text.to_string()` per provider call (`lsp.rs:129, 173, 201`, `state.read(cx).text().to_string()` at `251`), and the whole `apply_diagnostics` body: editor text clone, `to_string()` copy, per-diagnostic conversion, `DiagnosticSet` build (`lsp.rs:88-105`, entered from the UI closure at `session.rs:1578-1584`).
- **Level**: Confirmed (source); frame-time impact unmeasured.
- **Impact**: the architecture claim "the GPUI layer never blocks on a server" holds for I/O, but not for document copying or diagnostic projection — those scale with file size and diagnostic count on the render thread.

#### LSP-37 — 32 MiB frames parsed on a 2-worker runtime
- **Evidence**: `MAX_MESSAGE_BYTES = 32 MiB` (`framing.rs:12`); parse + `to_vec` + `drain` (LSP-14) on `astrolith-service` workers, `worker_threads(2)` (`runtime.rs:53-58`).
- **Level**: Confirmed (source); reachability needs a pathological payload.
- **Impact**: one huge frame can occupy half the service reactor, delaying document saves and preview decodes queued behind it.

#### LSP-38 — stderr pump has no line-length bound and outlives the client
- **Evidence**: detached task reads `BufReader::lines()` into `tracing::debug!` (`client.rs:283-290`); `AsyncBufReadExt::lines` allocates the whole line, and the task is not tracked by `AbortOnDrop` (which only covers the protocol actor, `client.rs:339-352`).
- **Level**: Confirmed.
- **Impact**: a server printing one enormous line allocates it whole; the pump ends only when the killed process closes stderr, so it can outlive `Client::drop` by a scheduling tick.

## Docs vs code

| Claim | Verdict |
|---|---|
| `editor-capabilities.md`: discovery prefers `node_modules/.bin` incl. hoisting, then `PATH`, never installs | accurate (`discovery.rs:78-95`) |
| `editor-capabilities.md`: full-text `didChange`, 150 ms coalescing | accurate (`client.rs:717-725`, `session.rs:1661`) |
| `editor-capabilities.md`: diagnostics, completion, hover, definition, code action; code action applies only current-document plain edits | accurate but understates LSP-30 (resolve/command actions silently no-op) and LSP-26 (no uri/version filter) |
| `editor-capabilities.md`: mode/tab/close stops the process | accurate; the stop is a SIGKILL without `shutdown`/`exit` (LSP-02) |
| `editor-capabilities.md` "not a full IDE": no rename / workspace symbol / signature help / diagnostics panel / inlay hints / semantic tokens / snippets / `documentChanges` / cross-file code actions | accurate — verified feature-by-feature in the matrix |
| `ARCHITECTURE.md:122`: servers start lazily per document language and project root; only the active Build document keeps a process | accurate (`session.rs:1480-1493`, `project_discovery.rs:3664-3670`) |
| `ARCHITECTURE.md:23`: `astrolith-lsp` owns protocol, process and document sync; `editor/lsp.rs` only adapts capabilities | accurate |
| `language.rs:23-25`: server identity exists for "one-server-per-language reuse" | contradicted — no reuse (LSP-01) |
| `concurrency-and-responsiveness.md`: 2 workers / 4 blocking threads / 2 blocking jobs | accurate for `runtime.rs`; LSP does not use the blocking pool (LSP-34) |

## Open questions / not inspected

1. **Real-server behaviour** decides severity for LSP-05, LSP-12, LSP-15, LSP-26, LSP-30, LSP-31, LSP-33: does astro-ls publish for unopened URIs, require `workspace/configuration`, return resolve-style actions, and accept full-text changes when it declared `Incremental`? No harness exists — `crates/astrolith-lsp/` has no `tests/` directory, only inline unit tests (`client.rs:904-1145`, `framing.rs:157-229`, `discovery.rs:203-291`, `position.rs:126-274`), which use `tokio::io::duplex` instead of a process.
2. **Per-keystroke cost** of LSP-21 (one document copy + one request per typed character) on a 1-5k-line `.astro` file: unmeasured here; a `ps`/tracing measurement of one session would settle it.
3. **Whether diagnostic projection stalls frames** in practice: `apply_diagnostics` is O(document) copies + O(#diagnostics × line) conversions on the UI thread, but no profile exists.
4. **Post-`uninstall` render safety**: whether a `DiagnosticSet` built against an older rope can survive `diagnostics_mut().clear()` + `refresh` (`lsp.rs:76-80`) into a paint pass. The paint path was not traced.
5. **`convert_positions` false positives**: any server field shaped `{start:{line,character}, end:{...}}` that is not a document range would be rewritten. Not found in the four integrated features, not exhaustively checked against astro-ls payloads.
6. **Process count under multi-document use** (LSP-01): inferred from code; no scenario in the repo opens two Build documents of the same language at once, so it was not observed with `ps`.
