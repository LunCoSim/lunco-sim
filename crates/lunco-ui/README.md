# lunco-ui

Reusable UI mechanisms for LunCoSim domain crates.

## What This Crate Does

`lunco-ui` provides the **infrastructure** that domain crates use to build panels. It does **not** contain panel implementations — those live in `src/ui/` of each domain crate (`lunco-modelica`, `lunco-sandbox-edit`, etc.).

### Architecture: Entity Viewers

All panels are **entity viewers** — they watch a selected entity and render its data. The same panel works in a standalone workbench, a 3D overlay, or a mission dashboard.

```
   Domain crate (lunco-modelica, lunco-fsw, etc.)
     ├── Defines entity component (ModelicaModel, FswConfig, etc.)
     ├── Defines viewer panel (DiagramPanel, CodeEditor, etc.)
     └── Panel watches WorkbenchState.selected_entity
           │
           ▼
   lunco-ui
     ├── Re-exports egui-snarl types for node graphs
     ├── Provides WidgetSystem for cached widget rendering
     └── Provides WorldPanel for 3D space panels
```

See `docs/research-ui-ux-architecture.md` in the workspace root for full architecture research.

### What's Provided

| Mechanism | Purpose |
|-----------|---------|
| **WidgetSystem** | O(1) cached ECS widgets for large-scale panels |
| **CommandBuilder** | AI-native UI — all interactions via `CommandMessage` |
| **WorldPanel** | 3D in-scene UI panels attached to entities |
| **Label3D** | Floating labels over 3D objects with LOD fade |
| **Time-series plots** | Zero-copy chart rendering via `egui_plot` |
| **Node graph types** | Re-exports `egui-snarl` (InPin, OutPin, Snarl, SnarlViewer) |

### What's NOT Here

- Panel implementations → domain crate `src/ui/`
- Docking system → `bevy_workbench`
- Theming → `bevy_workbench`
- Inspector/console → `bevy_workbench`

## Quick Start

### Add to Your Domain Crate

```toml
[dependencies]
lunco-ui = { path = "../lunco-ui" }
bevy_workbench = { workspace = true }
```

### Create a Panel

```rust
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_workbench::dock::WorkbenchPanel;
use lunco_ui::prelude::*;

pub struct MyPanel;

impl WorkbenchPanel for MyPanel {
    fn id(&self) -> &str { "my_panel_preview" }  // "preview" → center tab
    fn title(&self) -> String { "My Panel".into() }
    fn needs_world(&self) -> bool { true }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let selected = world.resource::<WorkbenchState>().selected_entity;
        // Render data for selected entity
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

- [ARCHITECTURE.md](ARCHITECTURE.md) — detailed mechanisms (WidgetSystem, CommandBuilder, 3D UI)
- [Workspace UI/UX Research](../../docs/research-ui-ux-architecture.md) — professional tool analysis and architecture decisions
