# lunco-ui

Reusable UI mechanisms for LunCoSim domain crates.

## What This Crate Does

`lunco-ui` provides the **infrastructure** that domain crates use to build panels. It does **not** contain panel implementations — those live in `src/ui/` of each domain crate (`lunco-modelica`, `lunco-sandbox-edit`, etc.).

### Architecture: Entity Viewers

All panels are **entity viewers** — they watch a selected entity and render its data. The same panel works in a standalone workbench, a 3D overlay, or a mission dashboard.

```
   Domain crate (lunco-modelica, lunco-mobility, etc.)
     ├── Defines entity component (ModelicaModel, FswConfig, etc.)
     ├── Defines viewer panel (DiagramPanel, CodeEditor, etc.)
     └── Panel watches WorkbenchState.selected_entity
           │
           ▼
   lunco-ui
     ├── Provides WidgetSystem for cached widget rendering
     └── Provides WorldPanel for 3D space panels
```

See [`docs/architecture/research/ui-ux-inspiration.md`](../../docs/architecture/research/ui-ux-inspiration.md) for full architecture research.

### What's Provided

| Mechanism | Purpose |
|-----------|---------|
| **WidgetSystem** | O(1) cached ECS widgets for large-scale panels |
| **Typed Commands** | AI-native UI — all interactions via Bevy ECS command events |
| **WorldPanel** | 3D in-scene UI panels attached to entities |
| **Label3D** | Floating labels over 3D objects with LOD fade |
| **Time-series plots** | Zero-copy chart rendering via `egui_plot` |
| **Node graphs / diagrams** | Render via `lunco-canvas`; domain crates own their projector |

### What's NOT Here

- Panel implementations → domain crate `src/ui/`
- Docking system → `lunco-workbench`
- Theming → `lunco-workbench`
- Inspector/console → `lunco-workbench`

## Quick Start

### Add to Your Domain Crate

```toml
[dependencies]
lunco-ui = { path = "../lunco-ui" }
lunco-workbench = { path = "../lunco-workbench" }
```

### Create a Panel

```rust
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
use lunco_ui::prelude::*;

pub struct MyPanel;

impl Panel for MyPanel {
    fn id(&self) -> PanelId { PanelId("my_panel_inspector") } // "inspector" → right dock
    fn title(&self) -> String { "My Panel".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    // Reads go through the capability-narrowed `PanelCtx` (no raw `&mut World`);
    // queue mutations with `ctx.defer(|world| { ... })`.
    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // e.g. `let sel = ctx.resource::<SelectedEntities>();`
    }
}
```

### Register It

```rust
// In your UI plugin:
app.register_panel(MyPanel);
```

### Panel ID Conventions (Auto-Slot)

| ID Contains | Auto-Slot | Position |
|-------------|-----------|----------|
| `"inspector"` | Right | Right dock |
| `"console"` or `"timeline"` | Bottom | Bottom dock |
| `"preview"` | Center | Center tab |
| (nothing matches) | Left | Left dock |

## See Also

- [ARCHITECTURE.md](ARCHITECTURE.md) — detailed mechanisms (WidgetSystem, typed commands, 3D UI)
- [Workspace UI/UX Research](../../docs/architecture/research/ui-ux-inspiration.md) — professional tool analysis and architecture decisions
