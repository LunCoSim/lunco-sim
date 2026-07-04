# `lunco-ui` — Reusable UI Mechanisms for LunCoSim

## Overview

`lunco-ui` provides **reusable mechanisms** that domain crates use to build panels. It does **not** contain panel implementations — those live in `src/ui/` of each domain crate.

## What lunco-ui Provides

> **Note:** the external `bevy_workbench` crate was replaced by the in-house
> `lunco-workbench` (path dep). The panel trait is now `Panel` (with `PanelCtx` /
> `PanelId`), *not* `WorkbenchPanel`. Some code snippets below still show the old
> trait name and are illustrative — check `lunco-workbench/src/panel.rs` for the
> current API.

```
┌─────────────────────────────────────────────────────────────┐
│                   lunco-workbench (in-house)                 │
│  Docking · Themes · Persistence · Inspector · Console        │
└──────────────────────┬──────────────────────────────────────┘
                       │ Panel trait
         ┌─────────────┼─────────────┐
  ┌──────▼──────┐ ┌───▼──────┐ ┌───▼──────────┐
  │lunco-sandbox│ │sandbox-  │ │lunco-        │
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
               │  Typed Commands │  ← AI-native command bus
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

### Typed Commands — AI-Native UI

All UI interactions dispatch typed commands (which are Bevy ECS events). Never mutate state directly from the UI.

```rust
if ui.button("Focus").clicked() {
    ctx.trigger(FocusTarget { target: body_entity });
}
```

This makes the UI AI-native: AI observes the same command events as humans, and can trigger identical commands.

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

**Node graphs / diagrams** — render on `lunco-canvas`. Domain crates own their node-type enum and projector; `lunco-canvas` handles pan/zoom/selection/draw and the workbench's animation hooks.

## Design Decisions

| Mechanism | What it gives us |
|-----------|-----------------|
| Docking (`lunco-workbench`) | Drag/drop panels, tabs, resize, undo — works out of the box |
| Themes (`lunco-workbench`) | Rerun Dark / Catppuccin — scientific dashboards look good immediately |
| Widget caching (`WidgetSystem`) | O(1) ECS queries for 1,000s of graph/diagram widgets |
| UI→State (Typed Commands) | All UI actions are observable, replayable, and AI-compatible |

## UI Decoupling Principle

**Panels never mutate state directly.** All UI interactions dispatch typed commands:

```
UI Panel (read-only query) ──Typed Command──▶ Observer (domain crate)
  "Focus Earth" button                         Focuses camera on entity
                                                ──Command Result (ACK)──▶ UI
```

| UI does | UI does NOT |
|---------|------------|
| Query state (read-only) | Mutate state directly |
| Emit typed commands | Call functions on resources |
| Display command results | Know about implementation details |

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
lunco-workbench = { path = "../lunco-workbench" }
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
            if let Some(target) = selected.entity {
                world.commands().trigger(DeleteEntity { target });
            }
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
#[on_command(DeleteEntity)]
fn on_delete_entity(trigger: On<DeleteEntity>, mut commands: Commands) {
    let cmd = trigger.event();
    // handle it
}

// In Plugin::build, register the command observer via register_commands!:
register_commands!(on_delete_entity);
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

Removing UI plugins leaves a functioning simulation. Headless binaries don't compile `lunco-workbench` or `bevy_egui`:

```rust
App::new()
    .add_plugins((MinimalPlugins, ScheduleRunnerPlugin::run_loop(...)))
    .add_plugins(LunCoAvatarPlugin)
    .add_plugins(SandboxEditPlugin)
    // No WorkbenchPlugin, no LuncoUiPlugin, no SandboxEditUiPlugin
    .run();
```

Existing integration tests (`lunco-avatar/tests/`) use this pattern.

## 3D UI LOD

`WorldPanel` and `Label3D` support distance-based fade/hide:

```rust
commands.spawn((
    Label3D {
        text: "Earth".into(),
        offset: DVec3::Y * (radius + 2000.0),
        billboard: true,
        lod: Some(WorldLod { fade_start: 1e7, fade_end: 5e7 }), // fade 10k–50k km
    },
    ChildOf(earth_entity),
));
```

The LOD system runs in `PostUpdate`, after transforms propagate. It hides widgets beyond `fade_end` to prevent visual clutter at large distances.

## Command Tracking

Result-returning commands return `Result<Ack, String>` (`Ok` for success/ACK, `Err` for failure/NACK), pollable by ID via `QueryCommandResult` for transport-dispatched calls (such as over the HTTP API or networked sessions).

In-process local UI triggers (`ctx.trigger` or `commands.trigger`) execute synchronously or trigger local state changes directly. The UI/AI can monitor outcome states by observing the respective domain state or listening to lifecycle events.

## File Structure

```
crates/lunco-ui/
├── ARCHITECTURE.md
├── Cargo.toml               # deps include bevy, bevy_egui, lunco-workbench, big_space, lunco-core, lunco-avatar
└── src/
    ├── lib.rs               # LuncoUiPlugin + theme
    ├── widget.rs            # WidgetSystem + WidgetId + caching
    ├── context.rs           # UiContext + UiSelection
    ├── helpers.rs           # collapsing tree row helpers
    ├── components.rs        # WorldPanel + Label3D
    ├── mission_control.rs   # mission-control panel widget
    ├── telemetry.rs         # telemetry panel widget
    ├── busy/                # busy/spinner indicator widget (mod.rs, spinner.rs, widget.rs)
    ├── diagrams/            # diagram widgets (mod.rs, time_series.rs)
    └── modal/              # modal-dialog host (mod.rs, host.rs)
```

Domain crate UI layout (panels implement `lunco-workbench`'s `Panel` trait):

```
crates/lunco-sandbox/src/ui/
├── mod.rs                   # sandbox UI plugin
├── code_panel.rs            # Panel impl
└── models_palette.rs        # Panel impl

crates/lunco-sandbox-edit/src/ui/
├── mod.rs                   # SandboxEditUiPlugin
├── spawn_palette.rs         # Panel impl
├── inspector.rs             # Panel impl
└── entity_list.rs           # Panel impl

crates/lunco-modelica/src/ui/
├── mod.rs                   # ModelicaUiPlugin
├── workbench.rs             # Panel impl
├── code_editor.rs           # Panel impl
└── graphs.rs                # Panel impl
```
