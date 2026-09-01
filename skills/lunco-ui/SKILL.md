---
name: lunco-ui
description: >
  LunCoSim UI architecture and panel implementation patterns.
  Use this skill whenever working on any user interface for the LunCoSim
  solar system simulation — adding panels, building dashboards, creating
  inspectors, spawning UI, telemetry displays, docking layouts, themes,
  or anything involving egui, lunco-workbench, or Panel.
  Also use when the user mentions typed commands, WidgetSystem, or 3D
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
3. **Panels are `Panel` impls** (the trait lives in `lunco_workbench`) — registered via `app.register_panel()` with lunco-workbench's docking system.
4. **Headless must work** — removing UI plugins (Layers 3 and 4) leaves a functioning simulation. See `AGENTS.md` §4.1 for the four-layer architecture.

The workbench status history is one shared presentation surface: render Info,
Progress, Warn, Error, and Attention through the same responsive
level/source/message/progress/action row. Its popup is compact, sized to roughly
half the available parent window and clamped to 420–960 logical px;
the message column consumes the remaining inner width rather than an arbitrary
fixed fraction. Diagnostic rows keep the shared column geometry, expose row
activation through the cursor and tooltip, and add the complete diagnostic as
an optional body under that row. Emit the existing typed action for Attention;
do not create level-specific row layouts or source-specific styling.

For Twin-browser work, use the workbench-owned `BrowserQuery` as the single
transient search field. Sections filter their own authoritative view-models by
human-readable names/paths, retain matching ancestors, and emit the existing
typed navigation actions. Do not add a per-domain search resource or make the
browser search generated ids.

Project-owned settings are not user-global settings: read the active Twin's
manifest through the workspace resource and emit a typed event for changes.
For the missing-asset consent flow, the popup's unchecked negative checkbox
means "show next time" and persists through `twin.toml [downloads]`; do not
add a second global settings key for it.

The Entity list follows the same rule for `ui.entity_list.grid_scope`: `current`
reads the authoritative `ActivePhysicsFrame`, `all` includes every mounted
BigSpace grid, and an omitted key uses the documented `current` default. The
Settings menu emits the existing generic `SetTwinSetting` command. Invalid
values are visible errors, and the derived tree is cleared on active
`TwinClosed`; do not cache this choice in a global UI resource or keep it across
Twin replacement.

Generated USD runtime scene edits use the same Twin-owned settings boundary:
`usd.runtime_persistence` is one boolean opt-in for both reading and writing
the `.lunco/runtime` cache. The Settings menu reads the USD owner's policy and
emits `SetTwinSetting`; it must not register a global autosave section or
invent a second persistence path.

Camera labels are a shared projection owned by
`lunco-usd-bevy::camera_switch::camera_display_labels`. Reuse it in workbench
camera lists, the USD/entity trees, and Inspector; keep the full USD path as
the selection/tooltip identity. Unique leaves are compact, duplicate leaves
gain nearest-owner context, generated ID suffixes stay out of primary text,
and only an unavoidable normalized collision gets an ordinal.

World-space vehicle trails are transient render presentation, not UI-owned state.
Read the vehicle root's solved Avian `Position` in the active physics/grid frame,
project through `GridSurfaceQuery`, and use bounded history with explicit
`SceneTeardown` cleanup. Do not derive trails from controller input, render
`GlobalTransform`, authored route geometry, or a per-frame USD edit; reuse the
shared ribbon mesh builder so turns and BigSpace frame changes use one geometry
contract.

## Runtime-authored HTML/CSS surfaces

For a Twin-facing HUD, telemetry card, progress overlay, or simple runtime
control that should change without a Rust rebuild, use the dedicated
[runtime-ui skill](../runtime-ui/SKILL.md) and
[`docs/architecture/runtime-authored-ui.md`](../../docs/architecture/runtime-authored-ui.md).

This is a separate presentation path built on HUI/Flair. It uses the generic
`EngineExposures` capability registry, the existing `WorkbenchEguiHost` camera,
and the workbench's authoritative dock/pick geometry. It does not replace
`Panel`/egui, create a second UI camera, or permit templates to mutate domain
state. Use this skill for workbench panels and use `runtime-ui` for authored
HTML/CSS surfaces; do not create a hybrid shim for one widget. The
`lunco-ui::modal` host is the canonical owner of queued modal outcomes,
scrim, focus, Esc dismissal, and typed `CloseModal` dispatch; HUI does not yet
supply those dialog semantics or checkbox/input state events.

The Modelica diagram's `Show nets` toggle is a workbench/egui presentation
setting stored on the per-tab `lunco-canvas::Canvas`. It hides rendered and
interactive connection edges without changing authored topology. Keep this
dynamic graph control in the canvas panel; HUI/Rhai does not provide the
dynamic list and graph-state contract it would require.

Canvas-owned diagram overlays must stay inside the owning leaf: direct painting
uses the canvas clip rectangle, while an `egui::Area` uses `constrain_to` with
the measured leaf rectangle. Non-interactive overlays do not claim pointer
input, and modal dialogs use the shared `lunco-ui::modal` host.

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
| Copy a SignalRegistry history every graph paint | Use the visualization-owned history fingerprint cache; copy only at the egui plot data boundary |
| Reproject every canvas edge every paint | Cache screen geometry by scene generation and viewport key; keep selection/tool state live |
| Walk the dock tree once per anchor group | Publish all slot anchors from one dock traversal |
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
crates/lunco-ui/           ← mechanisms (WidgetSystem, typed commands, 3D UI)
crates/lunco-*/src/ui/     ← domain-specific panels
```
