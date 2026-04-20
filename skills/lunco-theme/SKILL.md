---
name: lunco-theme
description: >
  LunCoSim's centralised theming system. Use this skill whenever you are
  about to write, touch, or review UI code that involves a color, spacing
  value, rounding, or egui visual style — in any panel, overlay, widget,
  gizmo label, or diagram. Trigger on any `Color32::from_rgb`, hex color,
  `ui.style_mut()`, `visuals.*`, `ctx.set_visuals`, "dark mode", "light
  mode", "accent color", "highlight", palette tweak, or mention of
  Catppuccin. Also trigger when adding a new panel that needs to pick
  colors, when a diagram needs port/connection colors, or when the user
  asks to "restyle", "retheme", or "make it match". The rules here are
  project-specific — defaults from egui or Bevy alone will lead you to
  hard-code colors, which violates the Tunability Mandate.
---

# LunCoSim Theming (`lunco-theme`)

Full API reference: `crates/lunco-theme/README.md`. This skill is the
decision guide for *where* colors/spacing come from in this repo.

## Hard rules

1. **No `Color32::from_rgb(...)`, hex literals, or RGBA tuples outside
   `crates/lunco-theme/`.** Every color in a panel, overlay, widget, or
   gizmo routes through the `Theme` resource. If the color you want
   doesn't exist yet, add a token — don't inline the value.
2. **Prefer semantic tokens over raw palette swatches.**
   - First choice: `theme.tokens.{accent, success, warning, error,
     success_subdued, text, text_subdued}`.
   - Second choice: `theme.colors.<catppuccin_name>` (`mantle`,
     `surface0..2`, `mauve`, `sapphire`, …) — only when no semantic
     token fits. Flag it as a follow-up to promote to a token.
   - Third choice: `theme.get_token(domain, token, fallback)` for
     domain-specific colors that don't belong in the global semantic
     set (e.g. `"modelica.diagram" / "port.fill"`). Register via
     `theme.register_override(domain, token, color)` in the domain's
     plugin `build`.
3. **Never call `ctx.set_visuals(...)` from a panel.**
   `lunco-ui::sync_theme_system` already pushes `theme.to_visuals()`
   to egui whenever `Theme` changes. For overlay-local tweaks that
   must not leak globally, mutate `ui.style_mut()` inside the overlay
   only — see `crates/lunco-sandbox-edit/src/overlay.rs::apply_theme`.
4. **Dark/light is `theme.toggle_mode()`, not a branch on
   `ThemeMode`.** Overrides survive the toggle automatically.
5. **Spacing and rounding come from `theme.spacing` and
   `theme.rounding`**, not ad-hoc `4.0` / `6.0` / `Margin::same(8.0)`
   literals.

## How to read `Theme`

### From a Bevy system

```rust
fn my_system(
    mut contexts: EguiContexts,
    theme: Res<lunco_theme::Theme>,
) {
    let ctx = contexts.ctx_mut().unwrap();
    egui::Area::new("x".into()).show(ctx, |ui| {
        ui.colored_label(theme.tokens.success, "ok");
    });
}
```

### From a `&mut World` widget / `WorkbenchPanel::ui_world`

Clone what you need out of `World` *before* touching `ui` — you can't
hold `Res<Theme>` and `&mut World` at the same time:

```rust
let (bg, tokens) = {
    let theme = world.resource::<lunco_theme::Theme>();
    (theme.colors.mantle, theme.tokens.clone())
};
// now render with bg + tokens; call back into world.commands() as usual
```

Canonical examples: `crates/lunco-client/src/models_palette.rs`,
`crates/lunco-sandbox-edit/src/ui/spawn_palette.rs`.

### Imports

```rust
use lunco_ui::prelude::{Theme, ThemeMode, ThemePlugin}; // re-exported
// or directly:
use lunco_theme::{Theme, ThemeMode, ThemePlugin, DesignTokens, ColorPalette};
```

## Picking the right token

| Need                           | Use                                    |
| ------------------------------ | -------------------------------------- |
| Primary/brand action           | `theme.tokens.accent`                  |
| Success / ok / online          | `theme.tokens.success`                 |
| Success button background      | `theme.tokens.success_subdued`         |
| Warning / caution              | `theme.tokens.warning`                 |
| Error / offline / destructive  | `theme.tokens.error`                   |
| Body text                      | `theme.tokens.text`                    |
| Secondary / muted text         | `theme.tokens.text_subdued`            |
| Panel background               | `theme.colors.mantle`                  |
| Window chrome                  | `theme.colors.crust`                   |
| Widget surface (resting)       | `theme.colors.surface0`                |
| Widget surface (hovered/active)| `theme.colors.surface1` / `surface2`   |
| Domain-specific decorator      | `theme.get_token(domain, token, …)`    |
| Selection highlight            | already in `to_visuals()` — don't redo |

If the answer is "none of these fit" — **add a semantic token to
`lunco-theme`** instead of reaching for a raw swatch. Semantic tokens
survive palette changes; swatches don't.

## Adding a new token

1. **Semantic (global)** — add a field to `DesignTokens` in
   `crates/lunco-theme/src/lib.rs`, derive it from the palette in
   `DesignTokens::from_palette`, and document what it's for. All
   themes get it for free.
2. **Domain-specific (local)** — register in the domain plugin's
   `build` using `theme.register_override("my.domain", "my.token",
   palette_color)`; read with `theme.get_token("my.domain",
   "my.token", fallback_color)`. No change to `lunco-theme`.

Pick semantic when the token is generic enough to be reused across
domains (e.g. `accent`); pick override when it's genuinely local
(e.g. a Modelica port color).

## Plugin wiring

- `lunco-workbench::WorkbenchPlugin` auto-adds `ThemePlugin` — full
  app shells get it for free.
- Headless UI tests or standalone panel harnesses: add it yourself,
  `app.add_plugins(lunco_theme::ThemePlugin)`. Without it,
  `Res<Theme>` will not be present and systems will panic on access.
- `lunco-ui::LuncoUiPlugin` installs `sync_theme_system`; add it
  wherever you want `Theme` changes to propagate to egui.

## Dark / light toggle

```rust
world.resource_mut::<lunco_theme::Theme>().toggle_mode();
```

- Preserves all registered overrides.
- The workbench status bar 🌙/☀ button already wires this — don't
  duplicate it in other panels.
- Don't branch on `theme.mode` in panel code to pick colors; pick the
  token and trust `Theme::dark()` / `Theme::light()` to have remapped
  it correctly.

## What NOT to do

| ❌ Don't                                             | ✅ Do                                              |
| ---------------------------------------------------- | -------------------------------------------------- |
| `Color32::from_rgb(46, 194, 126)`                    | `theme.tokens.success`                             |
| `ui.visuals_mut().override_text_color = Some(...)`   | Let `sync_theme_system` push `theme.to_visuals()`  |
| `if mode == Dark { red } else { dark_red }`          | One token; palette handles the swap                |
| Hard-code port colors in `canvas_diagram.rs`         | `theme.register_override` + `theme.get_token`      |
| `Margin::same(8.0)`                                  | `theme.spacing.window_padding`                     |
| Add a new `catppuccin-egui` dep in a domain crate    | Consume colors via `Theme`; version bridging lives in `lunco-theme` only |

## Review checklist

Before merging any UI change, scan the diff for:

- [ ] No new `Color32::from_rgb`, hex, or RGBA tuples outside `lunco-theme`.
- [ ] Every new color read goes through `theme.tokens.*`,
      `theme.colors.*`, or `theme.get_token(...)`.
- [ ] New domain-specific colors registered as overrides in the
      domain's plugin `build`, not inlined at call sites.
- [ ] No new `ctx.set_visuals` calls in panel code.
- [ ] Spacing/rounding pulled from `theme.spacing` / `theme.rounding`
      where a token exists.
- [ ] No `theme.mode == Dark` branches picking colors.

## Quick sanity check on an existing file

```bash
# Colors that should be routed through theme (ignore lunco-theme itself):
grep -rn "Color32::from_rgb\|Color32::from_rgba" crates/ \
  | grep -v "crates/lunco-theme/"

# ctx.set_visuals calls (should only be in lunco-ui's sync_theme_system):
grep -rn "set_visuals" crates/
```

Findings from either command are candidates to refactor into theme
tokens.
