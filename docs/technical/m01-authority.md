# M01 authority and execution decisions

Date: 2026-10-28.

The user reports that all M00 manual acceptance checks have passed: “m00我已手动验收均完善”. This is user-provided manual evidence, not a new instrumented result produced by the implementation agent. The historical M00 spike reports remain unchanged.

The referenced original design specification remains absent. After being told this, the user explicitly authorized implementation from the existing M01 plan: “授权，按现有 M01 计划继续”. Sections M01.1–M01.3 of the implementation plan govern this stage; this note does not reconstruct the missing overall specification.

## Scope and boundaries

- Add the stable shell in `astrolith-gpui`; keep desktop startup thin and preserve both disposable spike routes.
- Provide Light, Dark, and System appearance with centralized semantic colors. System follows native appearance changes; explicit choices do not.
- Use retained GPUI Kit dock regions for navigator, document, inspector, and optional bottom preview. Persist visibility and side widths, clamp restored dimensions, and report preference failures without losing usable state.
- One command registry supplies toolbar and searchable command-palette actions. Native shortcuts, palette confirmation/cancel, panel focus, and focus restoration receive interaction tests.
- M01 file navigation opens a local UTF-8 document as a readonly snapshot through a native file picker and background I/O. Astro discovery, editable document sessions, metadata forms, and saving remain owned by M02–M04. Opening a readonly file must never write it.
- Preferences are application settings, not document persistence. Use a versioned settings file and atomic replacement; serialize writes and prevent stale completion from replacing newer preferences. Preserve malformed or unsupported settings instead of silently overwriting them.

## Verification

Each task uses failing behavioral tests before implementation and ends with `mise run check` and a Conventional Commit. Run a bounded native shell smoke after integration. New M01 native appearance and interactions require their own evidence; the user's M00 acceptance does not automatically cover new code.

## Decisions within authorized scope

Keep the pinned `gpui-kit = "=0.8.1"` facade. Reuse Kit's Command, Dialog, DockSkin, and Editor rather than introducing new UI primitives. Use platform application-config locations for preferences. Persist preference changes independently of readonly document contents.
