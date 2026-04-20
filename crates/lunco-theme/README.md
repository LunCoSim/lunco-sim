# lunco-theme

Centralised design tokens for every LunCoSim UI surface. One Bevy `Resource`
(`Theme`) holds the active palette, semantic tokens, spacing, and rounding.
Every panel, overlay, or widget reads from it — **no hard-coded colors in
downstream crates**.

## What's inside

- **`Theme`** (`Resource`, `Clone`) — the single source of truth.
  - `mode: ThemeMode` (`Dark` | `Light`)
  - `colors: ColorPalette` — 26 Catppuccin swatches (`mauve`, `mantle`,
    `surface0`, …). Bridged to the workspace `egui::Color32` so egui
    version drift in `catppuccin-egui` can't leak upward.
    **Read from this tier only when defining a new semantic token.**
  - `tokens: DesignTokens` — **generic semantic colours** for any UI:
    `accent`, `success`, `warning`, `error`, `success_subdued`, `text`,
    `text_subdued`.
  - `schematic: SchematicTokens` — **schematic-editor semantic colours**
    (typed block-diagram editors, Modelica/SysML/etc.): wire colours by
    domain (electrical, mechanical, signal, thermal, fluid, …), class-
    kind badge backgrounds (model, block, package, …), schematic-
    panel text (muted / heading).
  - `spacing: SpacingScale` — `window_padding`, `item_spacing`, `button_padding`.
  - `rounding: RoundingScale` — `window`, `button`, `panel`.
  - `overrides: HashMap<(u64, u64), Color32>` — **user-customisation
    slot**: `register_override(domain, token, colour)` pins a value
    and detaches it from the palette. Use for theme-author or
    end-user customisation, **not** for defaults.
- **`ThemePlugin`** — registers `Theme` as a `Resource` (default = `dark`).
  Auto-added by `lunco-workbench`; add it yourself for headless-UI tests.

`lunco_theme::ThemePlugin`, `Theme`, and `ThemeMode` are re-exported from
`lunco_ui::prelude`, so most call sites import from there.

## Three token tiers

Pick the tier that matches the scope of the colour:

| Tier | Where | Populated by | Used for |
|------|-------|--------------|----------|
| **Generic semantic** | `theme.tokens.*` (`DesignTokens`) | `DesignTokens::from_palette` | Colours every UI uses — accent, success, error, body text |
| **Schematic-editor semantic** | `theme.schematic.*` (`SchematicTokens`) | `SchematicTokens::from_palette` | Colours any block-diagram editor uses — wire colours, class badges, diagram text |
| **Domain translation** | extension trait on `Theme` inside the domain crate | trait body maps domain types → schematic/generic tokens | Domain-specific translations ("a Modelica `Pin` is an electrical wire") |
| **User override** | `theme.overrides` via `register_override` / `get_token` | registered at runtime | Theme-author / end-user customisations that pin a colour and deliberately detach from the palette |

**Palette reads (`theme.colors.*`) are only legitimate inside the
`from_palette` builders, or inside a domain extension trait that's
deriving a schematic-token fallback via `get_token(domain, token,
default)`.** Never in a panel / overlay / widget.

## Using it

### 1. Plugin setup

```rust
// Full-app shell
app.add_plugins(lunco_workbench::WorkbenchPlugin); // adds ThemePlugin

// Or, in a headless UI harness / test:
app.add_plugins(lunco_theme::ThemePlugin);
```

`lunco-ui::LuncoUiPlugin` installs a `sync_theme_system` that pushes
`theme.to_visuals()` into the active egui context whenever the resource
changes — you don't need to call `ctx.set_visuals` yourself.

### 2. Reading in a system

```rust
fn draw_badge(
    mut contexts: EguiContexts,
    theme: Res<lunco_theme::Theme>,
) {
    let ctx = contexts.ctx_mut().unwrap();
    egui::Area::new("badge".into()).show(ctx, |ui| {
        ui.colored_label(theme.tokens.success, "ok");
        ui.colored_label(theme.schematic.wire_electrical, "bus");
    });
}
```

### 3. Reading inside a `&mut World` widget

Clone the fields you need out of `World` before touching `ui`:

```rust
let theme = {
    let t = world.resource::<lunco_theme::Theme>();
    t.clone() // whole struct clone; ~kilobyte, sub-µs
};
// ...render freely with `theme.tokens.*` / `theme.schematic.*`.
```

### 4. Styling an `egui::Ui` wholesale

`Theme::to_visuals()` returns a full `egui::Visuals` mapped from the
palette. `sync_theme_system` in `lunco-ui` calls this automatically on
change — don't reapply yourself.

### 5. Toggling dark/light

```rust
world.resource_mut::<lunco_theme::Theme>().toggle_mode();
```

`toggle_mode` preserves registered overrides across the swap. The
workbench status bar wires this to a 🌙/☀ button.

### 6. Domain-specific translation (the extension-trait pattern)

When a domain has type names (Modelica connector types, SysML stereotypes)
that need mapping to schematic colours, **define an extension trait on
`Theme` inside the domain crate** — never inline palette picks at call
sites.

```rust
// lunco-modelica/src/ui/theme.rs
use lunco_theme::Theme;
use bevy_egui::egui::Color32;

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
            ClassType::Block => self.schematic.class_block_badge,
            ClassType::Package => self.schematic.class_package_badge,
            // ...
        }
    }
}
```

Consumers then call `theme.wire_color("Pin")` and
`theme.class_badge_bg(&kind)` — zero palette picks in consumer code.

### 7. User overrides (pinning a specific value)

When a theme author or end-user wants to pin a colour that should
**not** track the palette on dark/light toggle:

```rust
theme.register_override("modelica.diagram", "port.fill", MY_CUSTOM_COLOR);
let color = theme.get_token("modelica.diagram", "port.fill", fallback);
```

This is the only legitimate use of `get_token` — resolving a pinned
override with a schematic-token fallback. Don't use it to "add a token
with a palette default"; that's what fields on `SchematicTokens` are
for.

## What to pick

| Need                          | Reach for                                       |
|-------------------------------|-------------------------------------------------|
| Primary action / brand        | `theme.tokens.accent`                           |
| Success / warning / error     | `theme.tokens.{success,warning,error}`          |
| Body / muted text             | `theme.tokens.{text,text_subdued}`              |
| Panel background              | `theme.colors.mantle`                           |
| Widget surface                | `theme.colors.surface0..surface2`               |
| Electrical wire               | `theme.schematic.wire_electrical`               |
| Mechanical flange             | `theme.schematic.wire_mechanical`               |
| Control signal (Real)         | `theme.schematic.wire_signal`                   |
| Class-kind badge              | `theme.schematic.class_{model,block,package,…}_badge` |
| Schematic muted / heading     | `theme.schematic.{text_muted,text_heading}`     |
| Domain type → schematic colour| extension trait in the domain crate             |
| User override                 | `theme.get_token(domain, token, fallback)` with prior `register_override` |

## Adding a new token

1. **Generic semantic (universal to all UI)** — add a field to
   `DesignTokens`, populate in `DesignTokens::from_palette`. All
   panels get it.
2. **Schematic-editor semantic** (any block-diagram editor) — add a
   field to `SchematicTokens`, populate in `SchematicTokens::from_palette`.
   All schematic domains (Modelica, SysML, electrical CAD) share it.
3. **Domain translation** (mapping a domain type name to an existing
   schematic token) — add a method to the domain's extension trait on
   `Theme`, returning `self.schematic.<field>`. Do **not** add a
   palette default; if the schematic token doesn't exist yet, add it
   in tier 2 first.
4. **User override** — no code change. Themes/users call
   `register_override`; consumer reads via `get_token`.

**Tier choice rule:** if you're about to write `self.colors.blue` as
a default inside tier 3, stop — you should be adding to tier 2 first.

## Non-goals

- **No hard-coded colors elsewhere.** If you find yourself typing
  `Color32::from_rgb(...)` in another crate, add a token here instead.
- **No egui-version coupling.** The palette stores workspace-`egui`
  `Color32`s, bridged from `catppuccin-egui` via component accessors so
  a minor-version mismatch doesn't break the build.
- **Not a general theming framework.** Scope is LunCoSim's own panels;
  we don't aim to theme third-party widgets beyond what
  `to_visuals()` covers.
