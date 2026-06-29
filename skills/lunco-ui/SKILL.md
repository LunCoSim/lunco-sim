---
name: lunco-ui
description: >
  LunCoSim UI architecture and panel implementation patterns.
  Use this skill whenever working on any user interface for the LunCoSim
  solar system simulation — adding panels, building dashboards, creating
  inspectors, spawning UI, telemetry displays, docking layouts, themes,
  or anything involving egui, lunco-workbench, or Panel.
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
2. **UI never mutates state** — all interactions emit typed command events (the `#[Command]` structs, triggered via `ctx.trigger(...)`) that observers handle. This makes the UI AI-native: AI observes the same command stream as humans and can emit identical commands.
3. **Panels are `Panel` impls** (the trait lives in `lunco_workbench`, the in-house replacement for the old external `bevy_workbench`) — registered via `app.register_panel()` with lunco-workbench's docking system.
4. **Headless must work** — removing UI plugins (Layers 3 and 4) leaves a functioning simulation. See `AGENTS.md` §4.1 for the four-layer architecture.

## Adding a Panel

```rust
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};
use lunco_ui::prelude::*;

pub struct MyPanel;

impl Panel for MyPanel {
    fn id(&self) -> PanelId { PanelId("my_panel") }
    fn title(&self) -> String { "My Panel".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        // READ state — view-model resources / selected components via the
        // ctx, never raw `&mut World` scans, never mutate.
        if let Some(sel) = ctx.resource::<UiSelection>() {
            // ... read sel ...
        }

        // EMIT a typed command event — never mutate directly.
        if ui.button("Action").clicked() {
            ctx.trigger(MyCommand { /* ... */ });
        }

        // Need `&mut World`? Queue it instead of blocking the paint:
        // ctx.defer(|world: &mut World| { /* ... */ });
    }
}

// Register in ui/mod.rs:
// app.register_panel(MyPanel);
```

## What NOT to Do

| ❌ Don't | ✅ Do |
|----------|------|
| Mutate resources directly from UI | `ctx.trigger(TypedCommand { .. })` (or `ctx.defer`), let observers handle it |
| Put UI code in `lib.rs` or outside `ui/` | All UI in `src/ui/` subdirectory |
| Use `world.query()` every frame for graphs | Use `WidgetSystem` for O(1) cached queries |
| Build custom docking/themes | Use lunco-workbench — it's already there |

## Discovering Existing Commands

Commands are typed structs marked `#[Command]`, handled by observers
marked `#[on_command(TypeName)]` (both from `lunco_core`). To find what
commands exist:

```bash
# Find all command observers
grep -rn "#\[on_command(" crates/

# Find all command struct definitions
grep -rn "#\[Command" crates/

# Find where a command is emitted from UI
grep -rn "\.trigger(" crates/
```

To add a new command: define a `#[Command]` struct + `#[on_command(..)]`
observer in the relevant domain crate (see the `test-via-api` skill's
"Add a command" section for the full pattern).

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
