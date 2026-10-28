# M00.3 — Theme and dock feasibility

2026-10-28. **GPUI Kit remains viable; layout acceptance and the M00/M01 gate remain open.** The real styled DockArea provides three retained regions, resizing, collapse/restore, and a standard themed Dialog without a replacement docking engine. Native pixels, keyboard ergonomics, and compositor scaling still require manual acceptance. The missing design specification is not reconstructed here; the M00 plan and shared spike evidence contract are provisional authority.

## Reproduce

```sh
mise install
mise run dev:layout
mise run test
mise run check
```

`--spike=layout` selects this disposable probe. `mise run dev` and `mise run dev:editor` retain their existing routes. The navigator and inspector start at 14 rem, the document receives surplus space. Drag either divider or use the labeled hide/show controls; DockSkin's native collapse controls also work. “Appearance…” opens the standard Dialog. Switch theme there, dismiss with Escape, and press Enter to reopen it from the restored trigger. Fixtures are embedded text; there is no persistence, file access, product model, or browser dependency.

## Environment and authority

- Locked Kit/Base/Component/assets 0.8.1 and GPUI `gpui-pre` 0.3.10; unchanged facade dependency/features: component/assets defaults, Markdown block and inline grammars; dev-only `test-support`. No lockfile changes.
- Mise 2026.10.19; Rust/Cargo 1.99.1; Node 26.12.1; Bun 1.4.4. Rust components remain exactly `rust-analyzer`, `rust-src`, `clippy`, `rustfmt`.
- CachyOS Linux 7.3.0-4-cachyos x86_64; Wayland; Hyprland 0.56.2 (`efb50993780079460b0cbed1363e2166a2de1d9f`). HDMI-A-1 is 2560×1440 at scale 1; eDP-1 is 2560×1600 at 1.3333334. Only HDMI-A-1 received this smoke launch; no physical scaling interaction was performed. Rendering adapter was not identified.
- Automated interaction evidence uses the GPUI test platform in debug mode, actual Kit Root, actual LayoutSpike, public snapshots and pointer/key dispatch. It does not inspect pixels or deliver OS keyboard input.

Context7 was first resolved to `/longbridge/gpui-kit`, then queried for DockSkin/DockArea, retained panels, resizing, themes and dialog focus. Current docs supported the direction; the exact APIs were checked in the installed `gpui-component-0.8.1/src/dock/{mod,panel,dock,tab_panel}.rs`, `theme/mod.rs`, `root.rs`, `window_ext.rs`, `button/button.rs`, and `gpui-base-0.8.1/src/dock/{dock_area,layout/builder}.rs`. Public test input/scale APIs were checked in Kit `src/test.rs` and GPUI `src/app/test_context.rs`. No crate-private test doubles or upstream patches are used.

## Automated results

Five UI integration tests pass:

| Test | Observed contract |
| --- | --- |
| `three_panels_have_nonzero_geometry_across_viewports_and_scales` | 800×600, 1000×700, 1440×900 logical viewports, each with simulated scale 1, 1.25, 1.5, 2. Actual panel bounds stay nonzero, horizontally ordered, nonoverlapping, and inside the viewport. |
| `dragging_each_native_handle_changes_only_its_panel` | Public input drags the observed left and right component handles by 40 logical pixels; only the intended side grows, while the opposite side retains its width. |
| `side_docks_hide_and_restore_through_visible_controls` | Both labeled controls remove the relevant panel, enlarge the document, and restore exact initial document bounds. |
| `native_collapse_control_keeps_toolbar_label_in_sync` | DockSkin's own left collapse button hides the navigator; the toolbar label updates, and the toolbar restores the native dock. |
| `theme_changes_with_dialog_open_and_escape_restores_trigger_focus` | Theme changes with the actual dialog open; primary token and solid-color projection agree. Escape dismisses it, focus returns to Appearance, Enter reopens it, and Shift+Tab reaches the toolbar theme control for keyboard activation. |

Observed logical geometry (navigator/document/inspector widths):

| Viewport | Widths | Content height across scales |
| --- | --- | --- |
| 800×600 | 224 / 352 / 224 | 500.5–501.6 |
| 1000×700 | 224 / 552 / 224 | 600.5–601.6 |
| 1440×900 | 224 / 992 / 224 | 800.5–801.6 |

Fractional logical origins (for example y=70.4 at scale 1.25) show scale-dependent layout rounding. These checks do not establish physical hairline alignment, glyph sharpness, or usable pointer targets on a compositor. 800×600 is the smallest tested geometry, **not** a verified minimum usable window size. Narrower windows and large application font zoom need product requirements and manual review.

RED→GREEN: the first four tests compiled against an empty real spike and failed on missing panel/trigger targets (0 passed, 4 failed). After implementation, the dialog test exposed missing trigger focus restoration while the other three passed. Pinned styled Button intentionally prevents pointer focus; Root captures the currently focused handle when opening. Retaining the trigger handle and explicitly focusing it before `open_dialog` fixed this without replacing Root's restoration/trap behavior. All four then passed. The fifth native-collapse integration test subsequently passed against that existing behavior. Final focused run: **5 passed**, 8 unrelated tests filtered. Final `mise run check`: **12 passed, 0 failed, 1 ignored** (the existing bounded editor performance diagnostic), with doctor/fmt/Clippy/build successful. `ci` is the same `check` composition; remote OS CI was not run.

The initial sandboxed setup/check attempts failed on Mise shim writes and doctor's `rustc` subprocess (`EPERM`). Retrying with installed-tool/cache access succeeded; no tool pin or Rust component changed.

## Native launch evidence

After the final build, a bounded subprocess procedure launched the binary once per layout/editor/default route, queried Hyprland clients filtered to that exact process PID, then terminated only that process. Each window mapped after approximately 0.83 seconds, `mapped=true`, `hidden=false`, `xwayland=false`, size 1263×686, monitor 0 (HDMI-A-1, scale 1), with empty stderr. Each attempt had a 12-second mapping limit and bounded cleanup. No compositor setting was changed. This is mapping evidence, not an image, keyboard, or timing acceptance test.

## API findings and limits

- `DockSkin::dock_area` attaches actual component chrome. A bare DockArea intentionally has no chrome. Keep the returned skin handle for appearance settings.
- Styled panel presentation must cross the Base seam through `panel_handle` and `DockLayout::panel_view`; a bare `.panel(entity)` supplies Base behavior but cannot expose the styled panel title methods to the skin.
- DockArea owns sizes, layouts and open state. `toggle_dock` and native collapse controls use that same retained state. The spike keeps no duplicate panel-visibility booleans.
- Dock sizing accepts pixels; initial side extents are resolved from rem at construction. This spike does not promise live rem-zoom persistence or resize restoration across restarts. The component's 100-pixel panel minimum and dock constraints are upstream behavior; no custom resizing arithmetic is introduced.
- `Theme::change` plus `Theme::update` is the centralized update path. The probe uses semantic surface/text/border/focus tokens and demonstrates a theme-level compact radius and primary-color customization. Updating through `global_mut` alone can leave component/background/Base projections stale. Custom spacing/elevation snapshots are not retained by the global theme API; they would need explicit ownership if later required.
- Standard Root/Dialog owns the overlay, trap, Escape, and restoration. A pointer-opened trigger must first establish the focus Root will capture. Builder closures only compose content; retained entities and focus are not recreated inside the builder.
- Pixel contrast, text wrapping/clipping, physical keyboard focus, native accessibility semantics, and actual compositor fractional scaling are unverified. No screenshots or physical-keyboard claims are made.

## Manual acceptance and decision

On each target native platform, record OS, compositor/backend, GPU/driver, display resolution/scale, font settings, window logical size, input devices, observed result, and capture evidence:

1. Run `mise run dev:layout`. Inspect all three full-height regions, titles, readable CJK/emoji text, hairlines, clipped content and focus rings at 1440×900, 1000×700 and 800×600. Shrink further to identify the actual usable minimum; hide auxiliaries when needed and confirm recovery paths remain visible.
2. Drag both dividers in each direction and to limits; confirm the opposite side remains stable and the document stays usable. Collapse and restore both side docks using toolbar and native DockSkin controls; repeat after resizing.
3. Tab/Shift+Tab through visible toolbar controls and panels. Activate hide/show and theme with the keyboard. Open Appearance by pointer and keyboard, switch theme repeatedly while open, check background/text/borders/controls for one consistent theme, then Escape and verify visible focus on Appearance and Enter reopening. Check focus cannot escape the active dialog.
4. Repeat steps 1–3 at **100%, 125%, 150%, 200% real display scale**, including moving the window between differently scaled displays. Inspect crisp glyphs/hairlines, hit targets, minimum size, overlay placement and focus. Simulated tests do not satisfy this step. Do not change compositor settings without the user's authorization.
5. Record each pass/failure. A zero-height or unreachable panel, lost restore path, wrong target resizing, stale theme, focus loss/trap leak, or unusable fractional-scale rendering is a no-go until an upstream fix or reviewed narrow adapter passes this matrix.

**Decision:** proceed only with independent M00 investigation. Keep GPUI Kit 0.8.1 as the candidate; no foundational control replacement is justified by these results. Physical/manual layout acceptance, the editor's native acceptance, and the missing-spec gate remain unresolved. **Do not begin M01.**
