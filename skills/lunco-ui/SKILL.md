---
name: lunco-ui
description: >
  LunCoSim UI architecture and panel implementation patterns.
  Use this skill whenever working on any user interface for the LunCoSim
  solar system simulation — adding panels, building dashboards, creating
  inspectors, spawning UI, telemetry displays, docking layouts, themes,
  or anything involving egui, bevy_workbench, or WorkbenchPanel.
  Also use when the user mentions CommandMessage, WidgetSystem, or 3D
  world-space UI. Even if the request seems simple (like "add a button"),
  use this skill because the panel registration and command patterns
  are project-specific and not obvious from Bevy alone.
---

# LunCoSim UI Architecture

## MANDATORY: Read Architecture First

Before implementing ANY UI, read:

```
crates/lunco-ui/ARCHITECTURE.md
```

It explains the full architecture, step-by-step panel integration guide,
and design decisions. This skill is a quick-reference summary.

## Core Principles

1. **UI lives in `src/ui/`** — domain crates have `src/ui/mod.rs` exporting a `*UiPlugin`. UI code never lives outside `ui/` directories.
2. **UI never mutates state** — all interactions emit `CommandMessage` events that observers handle. This makes the UI AI-native: AI observes the same command stream as humans and can emit identical commands.
3. **Panels are `WorkbenchPanel` impls** — registered via `app.register_panel()` with bevy_workbench's docking system.
4. **Headless must work** — removing UI plugins (Layers 3 and 4) leaves a functioning simulation. See `AGENTS.md` §4.1 for the four-layer architecture.

## Adding a Panel

```rust
use bevy_workbench::dock::WorkbenchPanel;
use lunco_ui::prelude::*;

pub struct MyPanel;

impl WorkbenchPanel for MyPanel {
    fn id(&self) -> &str { "my_panel" }
    fn title(&self) -> String { "My Panel".into() }
    fn needs_world(&self) -> bool { true }

    fn ui_world(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // READ state — query only, never mutate
        let selected = world.resource::<UiSelection>();

        // EMIT commands — never mutate directly
        if ui.button("Action").clicked() {
            world.commands().trigger(
                CommandBuilder::new("MY_COMMAND").target(entity).build()
            );
        }
    }
}

// Register in ui/mod.rs:
// app.register_panel(MyPanel);
```

## What NOT to Do

| ❌ Don't | ✅ Do |
|----------|------|
| Mutate resources directly from UI | Emit `CommandMessage`, let observers handle it |
| Put UI code in `lib.rs` or outside `ui/` | All UI in `src/ui/` subdirectory |
| Use `world.query()` every frame for graphs | Use `WidgetSystem` for O(1) cached queries |
| Build custom docking/themes | Use bevy_workbench — it's already there |

## Discovering Existing Commands

Commands are defined by observers that handle `CommandMessage` events.
To find what commands exist:

```bash
# Find all command observers
grep -rn "On<CommandMessage>" crates/

# Find all command names being matched
grep -rn 'cmd\.name ==' crates/

# Find all commands being emitted
grep -rn 'CommandMessage {' crates/
```

To add a new command, create an observer in the relevant domain crate.

## When to Use WidgetSystem

| Use `WidgetSystem` | Use raw queries |
|-------------------|-----------------|
| Queries same entities every frame | Reading 1-2 resources |
| 10+ query fields | Simple UI, minimal ECS |
| 100+ rendered items | Infrequent panels |

## File Structure

```
crates/lunco-ui/           ← mechanisms (WidgetSystem, CommandBuilder, 3D UI)
crates/lunco-*/src/ui/     ← domain-specific panels
```
