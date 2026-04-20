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
  - `tokens: DesignTokens` — **semantic** colors. Reach for these first:
    `accent`, `success`, `warning`, `error`, `success_subdued`, `text`,
    `text_subdued`.
  - `spacing: SpacingScale` — `window_padding`, `item_spacing`, `button_padding`.
  - `rounding: RoundingScale` — `window`, `button`, `panel`.
  - `overrides: HashMap<(u64, u64), Color32>` — per-domain fine-tuning.
- **`ThemePlugin`** — registers `Theme` as a `Resource` (default = `dark`).
  Auto-added by `lunco-workbench`; add it yourself for headless-UI tests.
- **`ColorPalette` / `DesignTokens`** — exposed for panels that need to
  clone the tokens out of `World` before rendering (common pattern with
  `&mut World` widgets).

`lunco_theme::ThemePlugin`, `Theme`, and `ThemeMode` are re-exported from
`lunco_ui::prelude`, so most call sites import from there.

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
    });
}
```

### 3. Reading inside a `&mut World` widget

Clone the fields you need out of `World` before touching `ui`:

```rust
let (bg, tokens) = {
    let theme = world.resource::<lunco_theme::Theme>();
    (theme.colors.mantle, theme.tokens.clone())
};
// ...then use `bg` and `tokens` freely.
```

See `crates/lunco-client/src/models_palette.rs` and
`crates/lunco-sandbox-edit/src/ui/spawn_palette.rs` for the canonical shape.

### 4. Styling an `egui::Ui` wholesale

Use `Theme::to_visuals()` to get a full `egui::Visuals` mapped from the
palette, or copy the pattern in
`crates/lunco-sandbox-edit/src/overlay.rs::apply_theme` for overlay-local
overrides that don't pollute the global context.

### 5. Toggling dark/light

```rust
world.resource_mut::<lunco_theme::Theme>().toggle_mode();
```

`toggle_mode` preserves registered overrides across the swap. The
workbench status bar wires this to a 🌙/☀ button (see
`crates/lunco-workbench/src/lib.rs`).

### 6. Per-domain overrides

When a domain needs colors not covered by the semantic tokens (e.g. a
diagram's port-fill palette), use the override registry instead of
branching on `ThemeMode`:

```rust
theme.register_override("modelica.diagram", "port.fill", theme.colors.sapphire);

let color = theme.get_token(
    "modelica.diagram",
    "port.fill",
    theme.colors.blue, // fallback if no override
);
```

Keys are hashed to `(u64, u64)` so lookups stay allocation-free on hot
paths.

## What to pick

| Need                         | Reach for                            |
| ---------------------------- | ------------------------------------ |
| "Primary action" color       | `theme.tokens.accent`                |
| Success / warning / error    | `theme.tokens.{success,warning,error}` |
| Body / muted text            | `theme.tokens.{text,text_subdued}`   |
| Panel background             | `theme.colors.mantle`                |
| Widget surface               | `theme.colors.surface0..surface2`    |
| Specific Catppuccin swatch   | `theme.colors.<name>` (last resort)  |
| Domain-specific decorator    | `theme.get_token(domain, token, …)`  |

Prefer semantic tokens over raw palette entries — they survive theme
changes without touching call sites.

## Non-goals

- **No hard-coded colors elsewhere.** If you find yourself typing
  `Color32::from_rgb(...)` in another crate, add a token here instead.
- **No egui-version coupling.** The palette stores workspace-`egui`
  `Color32`s, bridged from `catppuccin-egui` via component accessors so
  a minor-version mismatch doesn't break the build.
- **Not a general theming framework.** Scope is LunCoSim's own panels;
  we don't aim to theme third-party widgets beyond what
  `to_visuals()` covers.
