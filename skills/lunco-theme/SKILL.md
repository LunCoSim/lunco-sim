---
name: lunco-theme
description: >
  LunCoSim's centralised theming system. Use this skill whenever you are
  about to write, touch, or review UI code that involves a color, spacing
  value, rounding, or egui visual style — in any panel, overlay, widget,
  gizmo label, or diagram. Trigger on any `Color32::from_rgb`, hex color,
  `ui.style_mut()`, `visuals.*`, `ctx.set_visuals`, "dark mode", "light
  mode", "accent color", "highlight", palette tweak, mention of
  Catppuccin, or work on a typed block-diagram editor (wire colours,
  class-kind badges). Also trigger when adding a new panel that needs
  colors, or when the user asks to "restyle", "retheme", or "make it
  match". The rules here are project-specific — defaults from egui or
  Bevy alone will lead you to hard-code colors, which violates the
  Tunability Mandate.
---

# LunCoSim Theming (`lunco-theme`)

Full API reference: `crates/lunco-theme/README.md`. This skill is the
decision guide for *where* colors/spacing come from in this repo.

## Hard rules

1. **No `Color32::from_rgb(...)`, hex literals, or RGBA tuples outside
   `crates/lunco-theme/`.** Every color in a panel, overlay, widget,
   or gizmo routes through the `Theme` resource. If the color you
   want doesn't exist yet, **add a typed field** at the right tier —
   don't inline the value.
2. **Palette reads (`theme.colors.*`) only inside `from_palette`
   builders.** Anywhere else — including inside extension traits that
   provide defaults via `get_token` — is a smell. If the default you
   want is a palette entry, that's a sign you should be adding a
   field to `SchematicTokens` or `DesignTokens` first.
3. **Four tiers, pick the right one.** See [§ Tier guide](#tier-guide).
   Consumer code reads **fields**; `get_token` is reserved for
   resolving pinned user overrides.
4. **Never call `ctx.set_visuals(...)` from a panel.**
   `lunco-ui::sync_theme_system` already pushes `theme.to_visuals()`
   to egui whenever `Theme` changes.
5. **Dark/light is `theme.toggle_mode()`, not a branch on
   `ThemeMode`.** Overrides survive the toggle automatically.
6. **Spacing and rounding come from `theme.spacing` and
   `theme.rounding`**, not ad-hoc `4.0` / `6.0` / `Margin::same(8.0)`
   literals.

## Tier guide

Four tiers. Always work at the highest (most specific) tier that fits.
If you're tempted to hardcode a palette entry at a lower tier, you're
at the wrong tier — go up a level and add a field.

### Tier 1 — `DesignTokens` (generic semantic, universal to any UI)

Fields on `DesignTokens`, populated by `DesignTokens::from_palette`.
Colours *every* UI uses regardless of domain.

```rust
theme.tokens.accent
theme.tokens.success
theme.tokens.warning
theme.tokens.error
theme.tokens.text
theme.tokens.text_subdued
theme.tokens.success_subdued
```

Add a field here when the token is cross-cutting (e.g. a new
"destructive-action red" colour).

### Tier 2 — `SchematicTokens` (typed block-diagram editors)

Fields on `SchematicTokens`, populated by `SchematicTokens::from_palette`.
Colours that any schematic editor uses — Modelica, SysML, electrical
CAD, flow charts. Shared vocabulary, one palette→intent mapping.

```rust
// Wire colours by connector domain
theme.schematic.wire_electrical   // Pin, Plug
theme.schematic.wire_mechanical   // Flange
theme.schematic.wire_thermal      // HeatPort
theme.schematic.wire_fluid
theme.schematic.wire_signal       // RealInput/Output
theme.schematic.wire_boolean
theme.schematic.wire_integer
theme.schematic.wire_multibody    // Frame
theme.schematic.wire_unknown

// Class/component-kind badges
theme.schematic.class_model_badge
theme.schematic.class_block_badge
theme.schematic.class_class_badge
theme.schematic.class_connector_badge
theme.schematic.class_record_badge
theme.schematic.class_type_badge
theme.schematic.class_package_badge
theme.schematic.class_function_badge
theme.schematic.class_operator_badge
theme.schematic.class_badge_fg

// Schematic-panel typography
theme.schematic.text_muted
theme.schematic.text_heading
```

Add a field here when you need a new schematic concept (e.g. a
"selected wire" colour, a new connector domain).

### Tier 3 — Domain translation (extension trait)

A trait on `Theme` inside the domain crate. Maps domain-specific
names (Modelica `Pin`, parsed `ClassType::Model`, SysML stereotype)
to **tier 2 fields**. **Zero palette reads** in the trait body —
if the intent isn't in tier 2 yet, go add it there first.

```rust
// crates/lunco-modelica/src/ui/theme.rs
pub trait ModelicaThemeExt {
    fn wire_color(&self, connector_type: &str) -> Color32;
    fn class_badge_bg(&self, kind: &ClassType) -> Color32;
}

impl ModelicaThemeExt for Theme {
    fn wire_color(&self, connector_type: &str) -> Color32 {
        let leaf = connector_type.rsplit('.').next().unwrap_or(connector_type);
        let s = &self.schematic;
        match leaf {
            "Pin" | "Plug" => s.wire_electrical,
            "Flange_a" | "Flange_b" => s.wire_mechanical,
            "RealInput" | "RealOutput" => s.wire_signal,
            _ => s.wire_unknown,
        }
    }
    fn class_badge_bg(&self, kind: &ClassType) -> Color32 {
        match kind {
            ClassType::Model => self.schematic.class_model_badge,
            ClassType::Package => self.schematic.class_package_badge,
            // ...
        }
    }
}
```

### Tier 4 — User override (pin a value)

`theme.register_override(domain, token, colour)` + `theme.get_token(domain, token, fallback)`. Use **only** when:

- A theme author or end-user needs to pin a specific colour that
  should *not* track the palette on dark/light toggle, or
- You're resolving a historic token whose default is itself a tier 2
  field (e.g. `theme.get_token("modelica", "port_input", theme.schematic.wire_signal)`).

`get_token` is **not** the pattern for introducing new defaults.
If you're writing `self.colors.blue` as the fallback, stop — the
right fix is a new field in tier 2.

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
        ui.colored_label(theme.schematic.wire_electrical, "bus");
    });
}
```

### From a `&mut World` widget / `WorkbenchPanel::ui_world`

Clone the whole `Theme` out of `World` before touching `ui` — you
can't hold `Res<Theme>` and `&mut World` at the same time:

```rust
let theme = world.resource::<lunco_theme::Theme>().clone();
// now render freely with `theme.tokens.*`, `theme.schematic.*`,
// or (for Modelica) `theme.wire_color("Pin")` via the extension trait.
```

### Imports

```rust
use lunco_ui::prelude::{Theme, ThemeMode, ThemePlugin}; // re-exported
// or directly:
use lunco_theme::{Theme, ThemeMode, ThemePlugin, DesignTokens, SchematicTokens, ColorPalette};
// Domain extension (if any):
use crate::ui::theme::ModelicaThemeExt;
```

## Picking the right token

| Need                           | Use                                               |
|--------------------------------|---------------------------------------------------|
| Primary/brand action           | `theme.tokens.accent`                             |
| Success / ok / online          | `theme.tokens.success`                            |
| Warning / caution              | `theme.tokens.warning`                            |
| Error / offline / destructive  | `theme.tokens.error`                              |
| Body text                      | `theme.tokens.text`                               |
| Secondary / muted text         | `theme.tokens.text_subdued`                       |
| Panel background               | `theme.colors.mantle`                             |
| Widget surface                 | `theme.colors.surface0..surface2`                 |
| Electrical wire                | `theme.schematic.wire_electrical`                 |
| Mechanical flange              | `theme.schematic.wire_mechanical`                 |
| Signal (Real)                  | `theme.schematic.wire_signal`                     |
| Class-kind badge               | `theme.schematic.class_<kind>_badge`              |
| Schematic diagram muted text   | `theme.schematic.text_muted`                      |
| Domain type → schematic colour | extension trait (`theme.wire_color(…)`)           |
| User-pinned override           | `theme.get_token(...)` with prior `register_override` |

If the answer is "none of these fit" — **add a field in the right
tier**. Tier 1 if the colour is cross-UI; tier 2 if schematic-specific.
Don't default tier 3 with palette picks.

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

| ❌ Don't                                               | ✅ Do                                                    |
|-------------------------------------------------------|---------------------------------------------------------|
| `Color32::from_rgb(46, 194, 126)`                     | `theme.tokens.success`                                  |
| `theme.colors.blue` in a panel                        | Add a field to `SchematicTokens` or `DesignTokens`      |
| `self.colors.blue` as a default in an extension trait | `self.schematic.wire_electrical` field (add if missing) |
| `ui.visuals_mut().override_text_color = Some(...)`    | Let `sync_theme_system` push `theme.to_visuals()`       |
| `if mode == Dark { red } else { dark_red }`           | One token; palette handles the swap                     |
| `wire_color_for(connector)` local function per crate  | Domain extension trait returning `theme.schematic.wire_*` |
| `Margin::same(8.0)`                                   | `theme.spacing.window_padding`                          |
| Add a new `catppuccin-egui` dep in a domain crate     | Consume colors via `Theme`; bridging lives in `lunco-theme` only |

## Review checklist

Before merging any UI change, scan the diff for:

- [ ] No new `Color32::from_rgb`, hex, or RGBA tuples outside `lunco-theme`.
- [ ] No `theme.colors.*` reads outside `from_palette` builders or
      tier-4 `get_token` fallbacks.
- [ ] Every new colour goes through `theme.tokens.*`, `theme.schematic.*`,
      or a domain extension trait mapping domain types → those fields.
- [ ] New domain-specific colours modelled as extension-trait methods
      returning `theme.schematic.*` fields, not inlined at call sites.
- [ ] User-specific overrides registered via `register_override` in the
      domain plugin's `build`.
- [ ] No new `ctx.set_visuals` calls in panel code.
- [ ] Spacing/rounding pulled from `theme.spacing` / `theme.rounding`
      where a token exists.
- [ ] No `theme.mode == Dark` branches picking colors.

## Quick sanity check on an existing file

```bash
# Colors that should be routed through theme (ignore lunco-theme itself):
grep -rn "Color32::from_rgb\|Color32::from_rgba" crates/ \
  | grep -v "crates/lunco-theme/"

# Palette reads outside lunco-theme and from_palette builders:
grep -rn "theme\.colors\." crates/ \
  | grep -v "crates/lunco-theme/" \
  | grep -v "from_palette"

# ctx.set_visuals calls (should only be in lunco-ui's sync_theme_system):
grep -rn "set_visuals" crates/
```

Findings from any command are candidates to refactor into theme
tokens at the appropriate tier.
