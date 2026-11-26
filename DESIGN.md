# Astrolith Design System

> **Status**: Implemented on `dev` (`gpui-kit = "=0.9.0"`)  
> **Scope**: `crates/astrolith-gpui` and `apps/desktop`  
> **Normative Foundation**: Built on the GPUI Kit Design Guides and Coding Guides. When designing or modifying any UI surface in Astrolith, follow this document as the single source of truth alongside `AGENTS.md`.

---

## 1. Design Philosophy & Product Positioning

Astrolith is a native desktop writing and publishing studio for Astro sites, uniting three modes over one live project model:

- **Writing**: A calm, centered editorial desk for Markdown and MDX with transient Library and Frontmatter sheets.
- **Build**: A disciplined project workspace with a file explorer, multi-buffer tabs, source editor, Git Source Control sheet, and integrated terminal.
- **Preview**: A clean reading surface for Markdown/MDX paired with an Astro dev-server panel (external browser default on Wayland; embedded WebView optional).

### Core Principles — *Editorial Quiet Precision*

1. **Content leads; chrome recedes.** The document and collection hierarchy carry the visual focus. Adjacent surfaces separate via subtle tonal steps and 1 px hairlines rather than heavy borders, drop shadows, or nested cards.
2. **Desktop-native before web conventions.** Preserve keyboard workflows, explicit focus ownership, Escape dismissal, native menus, resizable docks, and region-owned scrollbars. Use the default arrow cursor for buttons, tabs, and list rows; reserve pointing-hand cursors strictly for external links.
3. **Tokens and relative scale (`rem`) before literals.** Every color resolves through `cx.theme()`. Every product layout length, height, gap, and dialog dimension scales from the window root font size (`rem`). Fixed `px(...)` values are permitted only for audited physical/platform boundaries (1 px hairlines, minimum window size, dock pixel APIs, measured PTY/editor cell metrics).
4. **Scarce emphasis.** Reserve `.primary()` strictly for the single default Enter-commit action in a decision area. Reserve semantic status colors (`success`, `warning`, `danger`, `info`) for active, meaningful states — never decoration or calm steady states.
5. **Concentric geometry & exact alignment spines.** Nested corners follow `inner ≈ max(0, outer − gap)`. Sibling headers, toolbars, rows, and footers lock onto shared leading/trailing edges.

---

## 2. Color & Theme System

Source of truth:
- [`crates/astrolith-gpui/src/theme/tokens.rs`](crates/astrolith-gpui/src/theme/tokens.rs)
- [`crates/astrolith-gpui/src/theme/palettes.rs`](crates/astrolith-gpui/src/theme/palettes.rs)
- [`crates/astrolith-gpui/src/theme/mod.rs`](crates/astrolith-gpui/src/theme/mod.rs)

All 14 built-in schemes (`AstrolithLight`, `AstrolithDark`, Catppuccin Latte/Frappé/Macchiato/Mocha, Nord/Nord Light, Tokyo Night/Storm/Moon/Day, Gruvbox Light/Dark) project through `theme::apply(...)` via `Theme::change` + `Theme::update(cx, ...)` so `colors`, `tokens`, and `gpui-base` scrollbars/handles stay synchronized.

### Surface Ladder

| Layer | Semantic Role | `AstrolithLight` | `AstrolithDark` | Usage |
|---|---|---|---|---|
| **Canvas** | `background` / `tab_active` | `#f7f8fa` | `#111214` | Writing canvas, source editor, active Build tab, reading preview |
| **Chrome** | `surface` (`sidebar`, `title_bar`, `status_bar`, `tab_bar`, `group_box`, `popover`) | `#ffffff` | `#181a1f` | Title bar, Build tab strip, sidebars, status bar, section boxes, floating popovers/sheets |
| **Segmented Track** | `tab_bar_segmented` | `surface.blend(fg @ 6.5%)` | `background.blend(#000 @ 32%)` | Recessed track behind the TitleBar mode switcher |
| **Raised Pill** | `elevated` | `#ffffff` | `surface.blend(fg @ 9%)` | Raised inner pill contrast against recessed tracks |
| **Structural Hairline** | `border` / `window_border` (`border_variant`) | `#dfe3ea` (`80%` variant) | `#2c303a` (`70%` variant) | Panel dividers (`window_border`) and control outlines (`border`) |
| **Primary Text / Muted** | `foreground` / `muted_foreground` | `#252b3a` (`68%` muted) | `#eaedf1` (`72%` muted) | Primary reading text and secondary metadata |
| **Primary Action** | `primary` | `#5267d8` | `#c8cdd6` | Default commit action and focus ring (`ring = primary`) |

### Semantic Status & Terminal Rules

- **Status roles**: `danger`, `warning`, `success`, `info`. Every status fill computes its foreground dynamically via relative luminance (`luminance > 0.18` → `#000000`, else `#ffffff`), guaranteeing ≥ 4.5:1 contrast across all 14 palettes, with `0.88` hover and `0.76` active opacity feedback.
- **Steady states stay neutral**: A saved document in `DocumentHeader` uses `muted_foreground`, not permanent `success` green.
- **Terminal ANSI readability**: `theme::terminal_ansi_palette(dark: bool) -> [u32; 16]` supplies separate dark-canvas and light-canvas 16-color ANSI palettes so terminal output stays legible in both appearances.
- **Zero raw colors in UI code**: Raw hex/`rgb`/`rgba`/`hsla` literals belong exclusively in `theme/palettes.rs` (and audited terminal ANSI/test fixtures). Never use palette hues like `cx.theme().red`; use `cx.theme().danger`.

---

## 3. Typography Scale

Source of truth: [`crates/astrolith-gpui/src/theme/typography.rs`](crates/astrolith-gpui/src/theme/typography.rs)

`theme.font_size` (default `14 px`, clamped `8..=28 px`) anchors `window.rem_size()` through `Root`:

| Role | Scale Helper | Font Family | Usage |
|---|---|---|---|
| **Micro / Meta** | `text_xs()` (`0.75rem`) | UI sans / mono | Panel headers, timestamps, form hints, status bar, shortcuts, badges |
| **Body / Control** | `text_sm()` (`0.875rem`) | UI sans (`Inter` → platform UI) | Buttons, inputs, list items, menus, document title, standard prose UI |
| **Prominent Control / Heading** | `text_base()` (`1rem`) | UI sans / `display_font` | Large segmented mode switcher, section headings, dialog headings |
| **Pane / Guide Title** | `text_lg()` (`1.125rem`) | `display_font` | Preferences pane headers, User Guide title |
| **Code & Tabular Data** | `text_xs()` / `text_sm()` | `theme.mono_font_family` | Source editor, terminal, git diffs, SHA-256 hashes, file paths, match counters, terminal dimensions (`80 × 24`) |
| **Document Prose** | Configured `ProseStyle` | `configured_prose_font` | Independent writing/preview typography (`font_size`, `line_height`, `page_width_percent`) |

- Use **sentence case** in English UI controls, menus, and headers. Avoid combining ALL CAPS with `font_semibold()` and strong accent color.

---

## 4. Spacing, Alignment & Scrolling

### Spatial Grammar (`rem` Helpers)

| Token Step | GPUI Helper | Typical Use |
|---|---|---|
| `xxs` (`0.125rem`) | `gap_0p5()`, `py_0p5()` | Optical corrections, compact chip vertical pad |
| `xs` (`0.25rem`) | `gap_1()`, `p_1()` | Icon/label inside compact controls, toolbar button inset |
| `sm` (`0.5rem`) | `gap_2()`, `p_2()` | Sibling controls, dialog footer buttons, icon/label in rows |
| `md` (`0.75rem`) | `gap_3()`, `px_3()` | Compact row groups, header groups |
| `lg` (`1rem`) | `gap_4()`, `p_4()`, `px_4()` | Standard panel header spine, sheet/panel body inset, form groups |
| `xl` (`1.5rem`) | `gap_6()`, `px_6()` | Major section separation in Preferences and inspectors |
| `xxl` (`2rem`) | `py_8()` | Empty-state breathing room |

### Structural Invariants

1. **Shared spines**: Panel headers (`ui::panel_header`) and panel bodies align on the `px_4()` spine; tree rows reserve a fixed centered disclosure lane (`w_3p5()`) and align cleanly with their header.
2. **Inside before outside (scrollbars)**: `.overflow_y_scrollbar()` owns the full edge of its panel or dialog body with zero outer padding. Content insets (`p_4()`, `px_6().py_5()`) live on an inner wrapper child so scrollbars sit flush against the region edge.
3. **Flex shrinking**: Any `flex_1()` container holding truncated text or scrollable children must declare `.min_w_0()` / `.min_h_0()`. Full-height columns inside `h_flex()` require `.items_stretch()` or `.h_full()`.

---

## 5. Border Radius & Elevation

### Concentric Radius Tiers

Derived from `settings.panel_radius` (clamped `0..=20 px`) in `theme::apply`:

| Tier | Accessor | Formula | Usage |
|---|---|---|---|
| **Surface (outer)** | `cx.theme().radius_lg` | `0` if `r == 0`, else `(r + 4).min(24)` | Dialogs, floating sheets, popovers (`ui::popover_surface`), empty-state icon wells, group containers |
| **Control (standard)** | `cx.theme().radius` | `r` | Buttons, inputs, list items, tabs |
| **Inner compact** | `ui::radius_sm(cx)` | `0` if `r <= 2`, else `(r − 2).max(2)` | Selection toolbar buttons inside `p_1()` popover, completion/code-action rows, removable chips, `Kbd` badges |
| **Full circle** | `rounded_full()` | `50%` | Strictly circular status dots (`ui::status_dot`) |

### Elevation Levels

- **Level 0 — Base chrome & canvas**: Flat (`background`, `sidebar`, `title_bar`, `status_bar`, `tab_bar`), separated by 1 px `window_border` hairlines. No shadows.
- **Level 1 — Grouped surfaces**: `group_box` cards and active segmented pills, defined by subtle tonal fill and 1 px `border` / `window_border`.
- **Level 2 — Anchored overlays**: `ui::popover_surface(cx)` (`bg(popover)` + `border_1().border_color(border)` + `rounded(radius_lg)` + `.shadow_lg()`). Used by Selection Toolbar, Alt-hover image preview, and all 5 SourceSurface overlays (completion, code action, hover, diagnostic, find/replace).
- **Level 3 — Modal sheets & dialogs**: `deferred(gpui_kit::base::Sheet)` and `window.open_dialog` / `open_alert_dialog` with `.shadow_xl()`, scrim backdrop, `.focus_trap()`, and Escape dismissal.

---

## 6. Layout Architecture

### Global Window Chrome (`shell.rs`)

- **Minimum window size**: `1000 × 700 px`.
- **TitleBar (`h(rems(3.5))`)**:
  - Leading (`flex_1().min_w_0()`): `ui::wordmark(cx)` + truncated project name + dismissable warning indicator with full-text tooltip.
  - Center: `TabBar::new("workspace-mode-switch").segmented().large()` (`36 px` track, `28 px` inner pill, `16 px` horizontal padding, `text_base()` labels) with combined shortcut tooltip (`Ctrl+Shift+1/2/3`).
  - Trailing (`flex_1().min_w_0().justify_end()`): `ui::icon_button_with_shortcut` for Command Palette and Preferences.
- **Writing Mode**:
  - `DocumentHeader`: Unified `2.75rem` rail (`RAIL_HEIGHT_REM`) in both empty and active states (zero height jump). Library toggle always sits on the leading `px_4()` spine and mirrors `library_open` via `.selected(...)`. Trailing cluster groups optional counts (`v_divider` rendered only when counts are shown), save indicator (`ui::status_dot` or `Spinner`), Save button (disabled when Saved/Saving), Focus/Typewriter/Inspector toggles (`.selected(...)` when active), and overflow menu.
  - Empty state updates immediately on `ProjectOpened` (`"Choose an entry from the library to begin writing."`).
- **Build Mode**:
  - Buffer tab strip uses `TabBar` / `Tab` on `cx.theme().tab_bar` (`surface` plane, matching TitleBar and Explorer) with path-keyed `ElementId`s, dirty dot prefix, close suffix button, and trailing `Source Control` / `Terminal` `.selected(...)` icon toggles.
- **Preview Mode**:
  - Centered `RenderedDocument` inside `size_full().v_flex()` (vertically centered `ui::empty_state` when no document is open), edge-mounted `gpui_kit::base::Scrollbar`, `.cursor_text()` canvas, and pointing-hand cursor strictly over hovered links.
- **Floating Sheets (Library, Frontmatter Inspector, Git Source Control)**:
  - All three use `deferred(gpui_kit::base::Sheet)` + `.focus_trap(...)` + Escape dismissal + shell-level mutual exclusion (opening one closes the others; dialogs take precedence).
  - Width: `rems(20.)..=rems(26.)` clamped to viewport. Top offset (`sheet_top`) adapts dynamically to visible chrome (`rems(6.)` in Build mode below TitleBar + Build tab strip, `rems(4.)` in Writing/Preview, `rems(0.75)` when writer chrome is auto-hidden).
- **Bottom Status Bar**: `gpui_kit::component::status_bar::StatusBar` (rem-scaled, dismissable transient status).

---

## 7. Shared UI Primitives (`crates/astrolith-gpui/src/ui.rs`)

Do not hand-roll repeated chrome in feature modules; compose from `crate::ui`:

| Primitive | Purpose & Contract |
|---|---|
| `mark(cx)` / `wordmark(cx)` | Appearance-aware SVG mark and `Astrolith` chrome title |
| `v_divider(cx)` | 1 px vertical chrome separator |
| `icon_button(id, icon, label)` | `.small().ghost()` icon button setting **both** `.tooltip(label)` and `.accessibility_label(label)` |
| `icon_button_with_shortcut(id, icon, label, shortcut)` | Same as `icon_button`, formatting `"Label (Shortcut)"` in tooltip |
| `panel_header(title, subtitle, trailing, cx)` | Unified `min_h(rems(2.5))` `px_4()` `window_border` header shared by FileBrowser, ProjectWorkspace, Preview, EmbeddedPreview, Inspector, and Git sheets |
| `empty_state(id, icon, title, body, action, cx)` | Full-height centered empty/error surface (`size_full().flex_1().min_h_0()`, `size_11()` icon well, `max_w(rems(20.))`) |
| `popover_surface(cx)` | Canonical Level-2 anchored overlay shell (`bg(popover)`, `border`, `radius_lg`, `shadow_lg()`) |
| `status_dot(color)` | `size_2().rounded_full().flex_shrink_0()` status dot |
| `inline_notice(id, tone, message) -> Alert` | Wraps `Alert::{info,success,warning,error}().small()`; callers chain `.on_close(...)` **only** when dismissable |
| `form_field_header(label, required, meta, trailing, cx)` | Unified form row header (`text_sm().font_medium().truncate()`, `danger` `*`, muted meta) |
| `removable_chip(id, label, remove_label, on_remove, cx)` | `min_h_6().py_0p5()` concentric chip (`radius_sm`) with accessible remove button |
| `kbd_hint(shortcut, cx)` | Platform-aware `Kbd::new(stroke).outline()` with muted monospace fallback |
| `dialog_footer(children)` | Right-aligned `h_flex().w_full().justify_end().gap_2()` action bar |
| `PickerOption` + `picker_button` / `picker_button_with` | `.small().outline().dropdown_caret(true)` dropdown menu button enforcing `.checked(selected)` and `.disabled(disabled)` on every item, with optional trigger `disabled` and `Rems`-bounded scrollable menu |
| `radius_sm(cx)` | Concentric inner corner radius helper |

### Component Choice Rules

- **Immediate boolean settings / frontmatter booleans**: `Switch` (not `"true"`/`"false"` `Checkbox` or `Off`/`On` dropdowns).
- **Consequential 2-button confirmations** (e.g. clean Move to Trash, Reset All): `window.open_alert_dialog` with `ButtonVariant::Danger` and specific object/verb labels (`Move to Trash`, `Cancel`).
- **Complex forms / preferences**: `Dialog` with actions pinned to `dialog.footer(...)` (never inside the scrollable body) and `InputEvent::PressEnter` wired to `.primary()` submit.
- **Metadata classification pills** (`MDX`, `Draft`): `Tag::secondary().outline().small()` / `Tag::warning().small()`.

---

## 8. Interaction States, Motion & Accessibility

### Interaction States

- Every interactive control visibly renders Rest, Hover, Pressed, Open/Selected (`.selected(bool)` on sheet/panel toggles and active tabs), Focus Visible (`ring` via `.track_focus(...)` + `.tab_stop(true)`), Disabled (`.disabled(true)` whenever an action cannot run), Loading (`Spinner::new().xsmall()` + disabled trigger while preserving surrounding context), and Validation/Error (inline per-field errors without closing dialogs or discarding user input).
- Selection is always communicated by surface fill (`.selected(bool)` / `ListItem::selected`), never by leading-edge accent bars.

### Motion

- Use `cx.theme().motion_tokens().duration_fast` and `ease_out_cubic` for subtle chrome reveal and overlay transitions; never hardcode raw `Duration::from_millis(...)` in views.
- Switching between already-loaded Preferences panes is **immediate** (zero artificial fade animation).

### Accessibility & Stable Identity

- Every icon-only button has an `.accessibility_label(...)` and `.tooltip(...)`.
- Every `Input` / `Textarea` has an `.aria_label(...)`.
- Every focusable panel/view registers `.tab_stop(true)` on its `FocusHandle` and wires `.track_focus(...)`.
- Every repeated `ElementId` derives from stable domain identity (canonical file path, frontmatter field path, component prop name, `CommandId` enum variant, collection name, version key) — **never** list indices or translated labels.
- Escape dismisses the topmost overlay/sheet/toolbar and restores focus to its trigger or editor.

---

## 9. Interface Language & Localization (`en`, `zh-CN`, `ja`)

All 10 locale files under [`crates/astrolith-gpui/locales/`](crates/astrolith-gpui/locales/) follow one product lexicon:

- **Ellipsis**: Always use the single character `…` (never `...`). Append `…` to triggers opening dialogs or sheets (`New entry…`, `User guide…`, `Keyboard shortcuts…`, `Privacy and local data…`).
- **Spelling & casing**: US English (`Color`, not `Colour`); sentence case in English UI; one-word `readonly` in identifiers, labels, and copy; zero internal jargon (`mock`, `atomically`, `argv`).
- **Button & dialog copy**: Specific result verbs (`Delete`, `Move to Trash`, `Discard changes`), `Cancel` consistently for leaving without committing.
- **Canonical `zh-CN` lexicon**: Trash = `废纸篓`, Library = `资料库`, Collection = `集合`, Frontmatter = `前置元数据`, Restart = `重启`, Reset all = `全部重置`, Preview auto-open = `进入预览模式时打开`, full-width punctuation `（）`.
- **Canonical `ja` lexicon**: Settings = `環境設定`, Browser = `ブラウザ`, Entry = `エントリ`, Focus mode = `フォーカスモード`, Preview auto-open = `プレビュー切り替え時に開く`.
