# M00 execution status

Date: 2026-10-28. Branch: `feat/m00-feasibility`.

## Completed and pending work

| Task | Implementation | Verification and acceptance |
| --- | --- | --- |
| M00.1 — toolchain and workspace | Committed as `294e2ea`; independent review approved. | Local doctor, formatting, Clippy, locked build/test, and native Wayland window mapping passed. Hosted macOS/Windows builds remain unverified. |
| M00.2 — editor spike | Committed as `fb6b2b0`; independent implementation review approved. | Seven interaction/capability tests passed. The separate release workload passed. Native visual, IME, clipboard, and presentation-latency acceptance remains open. |
| M00.3 — theme and dock spike | Committed as `b510c2a`; independent implementation review approved. | Five layout tests passed, including 12 size/scale combinations, both resize handles, native collapse, theme customization, and dialog focus. Native visual/scaling/focus acceptance remains open. |

The plan and review are under `docs/superpowers/plans/`; completed task checkboxes track verified progress. The referenced design specification is still missing. The implementation plan provisionally governs M00 feasibility work; these probes do not establish product architecture.

## Reproduce current results

```sh
mise install
mise run doctor
mise run ci
mise run dev:editor
mise run dev:layout
mise run test:editor-perf
```

Mise owns exact Rust, Node.js, and Bun versions. The Rust tool entry includes the complete required component list:

```toml
components = ["rust-analyzer", "rust-src", "clippy", "rustfmt"]
```

The default test suite runs without native windows. The explicit editor performance task uses optimized code and a headless component test platform. Native launch observations establish window mapping, rather than correct pixels, OS input behavior, or display latency.

The final combined `mise run check` passed with 12 tests, zero failures, and one explicitly skipped performance diagnostic. The editor diagnostic was run separately in release mode: four workloads and 400 finite input/frame samples passed. All three launch routes mapped native Wayland windows on the tested Linux machine. These results do not establish hosted or macOS/Windows CI success.

For exact APIs, measurements, limitations, and manual steps, see [the editor report](editor-spike.md) and [the theme/dock report](design-system-spike.md).

## Non-blocking review follow-up

Extend the layout geometry test with top/bottom viewport-containment assertions. It currently checks horizontal containment, positive panel dimensions, and ordering. This is a deferred test enhancement; review found no blocking implementation defect. Carry it into the eventual whole-branch review before integration.

## Gate before M01

The original automated session left M00 open. On 2026-10-28, the user reported that all manual acceptance was complete: “m00我已手动验收均完善”. This supplies the manual acceptance decision for M00; it does not add instrument traces or platform details to the earlier reports.

The user then explicitly authorized the existing M01 plan as the stage design authority despite the absent original specification: “授权，按现有 M01 计划继续”. See [the authority record](m01-authority.md). The following list records what required manual acceptance before that confirmation:

- Follow the native Chinese/Japanese preedit, commit/cancel, and refresh procedures in [the editor report](editor-spike.md).
- Inspect actual Markdown highlighting, Unicode glyphs, selection, and caret rendering.
- Verify clipboard interchange with another native application and keyboard/focus behavior.
- Measure actual native load and input-to-presentation performance. Headless CPU measurements are a separate result.
- Verify the theme/dock spike's actual appearance, overlay focus, and compositor scaling at representative window sizes, following [its native acceptance matrix](design-system-spike.md).
- Recover the design specification or explicitly reconcile the plan with the intended product design before product implementation.

Acceptance reports distinguish automated context tests, native mapping observations, and user-provided manual acceptance. Any later native regression requires investigation and a rerun; an editor replacement strategy still requires approval under the original plan.

The earlier schema-generation, source-mapping, lossless-patching, document-lifecycle, and MDX review findings remain requirements for their owning product milestones. This feasibility work does not close them.
