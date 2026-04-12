# `lunco-ui` — Reusable UI Mechanisms for LunCoSim

## Overview

`lunco-ui` provides **reusable mechanisms** that domain crates use to build panels. It does **not** contain panel implementations — those live in `src/ui/` of each domain crate.

## What lunco-ui Provides

```
┌─────────────────────────────────────────────────────────────┐
│                   bevy_workbench (external)                  │
│  Docking · Themes · Persistence · Inspector · Console        │
└──────────────────────┬──────────────────────────────────────┘
                       │ WorkbenchPanel trait
         ┌─────────────┼─────────────┐
  ┌──────▼──────┐ ┌───▼──────┐ ┌───▼──────────┐
  │lunco-client │ │sandbox-  │ │lunco-        │
  │  panels     │ │edit      │ │modelica      │
  │             │ │panels    │ │panels        │
  │ MissionCtl  │ │ SpawnPal │ │ Workbench    │
  │ Telemetry   │ │ Inspect  │ │ CodeEditor   │
  └──────┬──────┘ └───┬─────┘ └────┬─────────┘
         │             │            │
         └─────────────┼────────────┘
                       │ uses mechanisms from lunco-ui
              ┌────────▼────────┐
              │    lunco-ui     │
              │  WidgetSystem   │  ← O(1) cached ECS widgets
              │  CommandBuilder │  ← AI-native CommandMessage
              │  WorldPanel     │  ← 3D in-scene UI
              │  Label3D        │
              └─────────────────┘
```

### WidgetSystem — O(1) Cached ECS Widgets

For panels that query ECS data every frame (graphs, inspectors, large lists). Naive `world.query()` is O(n) per widget per frame — unacceptable at scale.

```rust
// Widget is a SystemParam — declares ECS access as fields
#[derive(SystemParam)]
struct TimeSeriesWidget<'w, 's> {
    channels: Res<'w, ModelicaChannels>,
    plotted:  Res<'w, PlottedVariables>,
    scroll:   Local<'s, f64>,   // persists across frames
}

// Implement WidgetSystem — render with cached state
impl WidgetSystem for TimeSeriesWidget<'_, '_> {
    fn run(world: &mut World, state: &mut SystemState<Self>, ui: &mut egui::Ui, id: WidgetId) {
        let mut params = state.get_mut(world);  // O(1) after first frame
        // render egui_plot
    }
}

// Called uniformly — same signature for ALL widgets
widget::<TimeSeriesWidget>(world, ui, WidgetId::new("graph").with(entity).with("velocity"));
```

**Performance**: First frame O(n) init, then O(1). 2,000 widgets ≈ 12ms/sec vs 6 sec/sec naive.

### CommandBuilder — AI-Native UI

All UI interactions flow through `CommandMessage`. Never mutate state directly from UI.

```rust
if ui.button("Focus").clicked() {
    ctx.trigger(CommandBuilder::new("FOCUS").target(body_entity).build());
}
```

This makes the UI AI-native: AI observes the same CommandMessage stream as humans, and can emit identical commands.

### 3D World-Space UI

Components for in-scene displays — nobody else provides this:

```rust
commands.spawn((
    Label3D { text: "Earth".into(), offset: DVec3::Y * (radius + 2000.0), billboard: true },
    ChildOf(earth_entity),
));
```

### Diagram Widgets

**Time-series charts** — `time_series_plot()` is a pure rendering function. Zero data copies: the domain panel borrows its data, wraps it in `ChartSeries` references, and passes to the widget.

```rust
// Domain panel borrows data, no copying
let series: Vec<ChartSeries> = plotted.names.iter()
    .filter_map(|name| channels.get(name).map(|ch| ChartSeries {
        name,
        y_values: ch.history.as_slice(),  // borrowed slice
        dt: Some(ch.dt),
        color: None,
    }))
    .collect();

time_series_plot(ui, "modelica_plot", &series);
```

**Node graphs** — re-exports `egui-snarl` types. Domain crates define their own node type and `SnarlViewer`, then call `snarl.show()`.

## Design Decisions

| Mechanism | What it gives us |
|-----------|-----------------|
| Docking (`bevy_workbench`) | Drag/drop panels, tabs, resize, undo — works out of the box |
| Themes (`bevy_workbench`) | Rerun Dark / Catppuccin — scientific dashboards look good immediately |
| Widget caching (`WidgetSystem`) | O(1) ECS queries for 1,000s of graph/diagram widgets |
| UI→State (`CommandMessage`) | All UI actions are observable, replayable, and AI-compatible |

## UI Decoupling Principle

**Panels never mutate state directly.** All UI interactions emit `CommandMessage`:

```
UI Panel (read-only query) ──CommandMessage──▶ Observer (domain crate)
  "Focus Earth" button                         Focuses camera on entity
                                                ──CommandResponse──▶ UI
```

| UI does | UI does NOT |
|---------|------------|
| Query state (read-only) | Mutate state directly |
| Emit CommandMessage | Call functions on resources |
| Display CommandResponse | Know about implementation details |

## How to Add UI to a Domain Crate

### 1. Organize files

```
crates/lunco-sandbox-edit/
├── src/
│   ├── lib.rs              # SandboxEditPlugin (logic only)
│   ├── spawn.rs
│   └── ui/                 # ALL UI — independent plugin
│       ├── mod.rs          # SandboxEditUiPlugin
│       ├── spawn_palette.rs
│       └── inspector.rs
```

### 2. Add dependencies

```toml
[dependencies]
bevy_workbench = "0.3"
lunco-ui = { path = "../lunco-ui" }
```

### 3. Implement a panel

```rust
use bevy_workbench::dock::WorkbenchPanel;
use lunco_ui::prelude::*;

pub struct Inspector;

impl WorkbenchPanel for Inspector {
    fn id(&self) -> &str { "sandbox_inspector" }
    fn title(&self) -> String { "Inspector".into() }
    fn needs_world(&self) -> bool { true }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // READ state — query only
        let selected = world.resource::<UiSelection>();

        // EMIT commands — never mutate
        if ui.button("Delete").clicked() {
            world.commands().trigger(
                CommandBuilder::new("DELETE_ENTITY")
                    .target(selected.entity?)
                    .build()
            );
        }
    }
}
```

### 4. Create the UI plugin

```rust
// ui/mod.rs
use bevy::prelude::*;
use bevy_workbench::WorkbenchApp;

pub mod spawn_palette;
pub mod inspector;

pub struct SandboxEditUiPlugin;

impl Plugin for SandboxEditUiPlugin {
    fn build(&self, app: &mut App) {
        app.register_panel(spawn_palette::SpawnPalette);
        app.register_panel(inspector::Inspector);
    }
}
```

### 5. Register in binary

```rust
// With UI:
app.add_plugins(SandboxEditPlugin)       // logic
   .add_plugins(WorkbenchPlugin::default())
   .add_plugins(LuncoUiPlugin)
   .add_plugins(SandboxEditUiPlugin)     // UI
   .run();

// Headless (no UI deps):
app.add_plugins(SandboxEditPlugin)
   .run();
```

### 6. Ensure observers handle commands

```rust
app.add_observer(on_delete_entity);

fn on_delete_entity(trigger: On<CommandMessage>, mut commands: Commands) {
    if trigger.event().name == "DELETE_ENTITY" {
        // handle it
    }
}
```

## When to Use WidgetSystem

| Use WidgetSystem | Use raw queries |
|-----------------|-----------------|
| Queries same entities every frame | Reading 1-2 resources |
| 10+ query fields | Simple UI, minimal ECS |
| 100+ rendered items | Infrequent panels |

## Existing Commands

| Command | Observer Location | Effect |
|---------|------------------|--------|
| `FOCUS` | lunco-avatar | Focus camera on target |
| `RELEASE` | lunco-avatar | Free-fly camera |
| `POSSESS` | lunco-avatar | Take control of vessel |
| `TELEPORT_SURFACE` | lunco-avatar | Teleport avatar to surface |
| `LEAVE_SURFACE` | lunco-avatar | Return to orbit |
| `DRIVE_ROVER` | lunco-mobility | Set wheel intents |
| `SPAWN_ENTITY` | lunco-sandbox-edit | Spawn catalog item |

## Headless

Removing UI plugins leaves a functioning simulation. Headless binaries don't compile `bevy_workbench` or `bevy_egui`:

```rust
App::new()
    .add_plugins((MinimalPlugins, ScheduleRunnerPlugin::run_loop(...)))
    .add_plugins(LunCoAvatarPlugin)
    .add_plugins(SandboxEditPlugin)
    // No WorkbenchPlugin, no LuncoUiPlugin, no SandboxEditUiPlugin
    .run();
```

Existing integration tests (`lunco-avatar/tests/`) use this pattern.

## File Structure

```
crates/lunco-ui/
├── ARCHITECTURE.md
├── Cargo.toml               # 7 deps (bevy, bevy_egui, bevy_workbench, big_space, lunco-core, lunco-avatar, smallvec)
└── src/
    ├── lib.rs               # LuncoUiPlugin (~20 lines)
    ├── widget.rs            # WidgetSystem + WidgetId + caching (~180 lines)
    ├── context.rs           # UiContext + UiSelection (~50 lines)
    ├── helpers.rs           # CommandBuilder (~50 lines)
    └── components.rs        # WorldPanel + Label3D (~30 lines)
    # Total: ~330 lines
```

Domain crate UI layout:

```
crates/lunco-client/src/ui/
├── mod.rs                   # ClientUiPlugin
├── mission_control.rs       # WorkbenchPanel impl
└── telemetry.rs             # WorkbenchPanel impl

crates/lunco-sandbox-edit/src/ui/
├── mod.rs                   # SandboxEditUiPlugin
├── spawn_palette.rs         # WorkbenchPanel impl
├── inspector.rs             # WorkbenchPanel impl
└── entity_list.rs           # WorkbenchPanel impl

crates/lunco-modelica/src/ui/
├── mod.rs                   # ModelicaUiPlugin
├── workbench.rs             # WorkbenchPanel impl
├── code_editor.rs           # WorkbenchPanel impl
└── graphs.rs                # WorkbenchPanel impl
```
