# Sandbox Control UI — Implementation Plan (light-core)

Adds a meaningful control surface to the sandbox for: possession/selection
status, autopilot/behaviour-tree authoring, checkpoints (Alt+LMB), right-click
context menus, and rhai-defined rover activities ("take photo"). Follows the
four-layer plugin architecture (§4.1), typed commands (§4.2), tunability
mandate (§3), frame discipline (§7), TDD (§1), headless-first (§4.1-4).

## Core design (keep Rust light, author in rhai/USD)

**Checkpoint state = the behaviour tree.** A checkpoint list is a
`BehaviorSpec::Patrol { waypoints, speed, radius, dwell }` (already in
`lunco-autopilot`). The vessel's `AutopilotBehavior` (compiled tree) is the
runtime instance; `SetAutopilotBehavior` (existing `#[Command]`, reachable from
rhai/API/UI) is the single mutation seam. Persistence of an authored patrol
lives where the behaviour lives — a rhai scenario (`ScriptDocument`, already
journaled `DomainKind::Script`) or a USD prim's metadata — **no new
`DomainKind`, no new `CheckpointRegistry`, no new typed Rust commands** for
checkpoints. The Rust core only gains a small inspection component so the UI
can read/visualize the tree.

## Steps

1. **`AutopilotBehaviorSpec` component** (`lunco-autopilot`): stores the source
   `BehaviorSpec` alongside the compiled `AutopilotBehavior`, set in
   `on_engage_autopilot` / `on_set_autopilot_behavior`. Exposes
   `waypoints_of(vessel)` for the UI. TDD: `tests/behavior_spec_test.rs`.

2. **rhai mission prelude** (`assets/scripting/prelude/mission.rhai`):
   `patrol(vessel, points, speed?, radius?, dwell?)`,
   `add_checkpoint(vessel, x, y, z)`, `engage_patrol(vessel)`,
   `clear_patrol(vessel)` — all wrappers that build a `BehaviorSpec` JSON and
   call the existing `cmd("SetAutopilotBehavior", …)`. No Rust changes.

3. **Checkpoint path-line gizmo** (`lunco-sandbox-edit/src/ui/checkpoint_gizmo.rs`,
   ui-gated `CheckpointGizmoPlugin`): reads `AutopilotBehaviorSpec` +
   `GlobalTransform` → numbered pins + connecting lines via `Gizmos`, themed
   via `lunco_theme::Theme`. Change-gated (run only when the spec or vessel
   count changes, §7).

4. **Alt+LMB + right-click observer** (`lunco-sandbox-edit/src/ui/checkpoint_click.rs`,
   ui-gated): raycasts terrain via `spawn::terrain_ray_hit`, resolves the
   primary selection, appends/removes a waypoint, and re-triggers the
   **existing** `SetAutopilotBehavior` command with the new spec JSON. Right-
   click opens an egui popup (Delete). No new verb — reuses `SetAutopilotBehavior`.

5. **`CommandDeckPanel`** (`lunco-sandbox-edit/src/ui/command_deck.rs`): reads
   `SelectedEntities` + `AutopilotBehaviorSpec` via a change-gated
   `CommandDeckView`; emits `PossessVessel`/`ReleaseVessel`,
   `EngageAutopilot`/`SetAutopilotBehavior`, "clear patrol". Lists checkpoints
   (from the spec) with delete buttons.

6. **`AvatarStatusView` possessed-vessel readout** (`lunco-avatar`): extend the
   existing producer (src/ui/mod.rs) to resolve `ControllerLink.vessel_entity`
   → `GlobalEntityId` → name; "Driving: <name>" or "Free flight".

7. **`CaptureFromCamera` + rhai `photo()`** (`lunco-avatar/src/screenshot.rs`):
   a sibling typed command to `CaptureScreenshot` that targets a vessel's
   `def Camera`. rhai prelude `control.rhai` gains `fn photo()` /
   `fn photo_from(vessel)`.

8. **`BehaviorSpec::RunTool` leaf + `science.rhai`** (`lunco-autopilot`):
   a new behaviour-tree leaf firing a registered tool call (e.g.
   `science::take_photo`); lets a patrol sequence include "take photo at
   waypoint N". Updates `build_tree` and the btcpp_xml codec round-trip.

## Constraints checklist

- [x] No magic numbers — marker size, colour, default patrol speed/radius/dwell
      live in a `Resource` / `Theme` (§3). `CheckpointGizmoSettings` (pin size,
      pick radius) + `PatrolDefaults` (speed/radius/dwell/engage-throttle) are
      the two tunable resources.
- [x] Layer 2 vs Layer 4 split: domain logic in `src/`, UI in `src/ui/` (§4.1).
- [x] UI never mutates state directly — dispatches typed `#[Command]`s (§4.2).
      Checkpoints reuse `SetAutopilotBehavior`; the new `ClearPatrol` command is
      the single "stop & clear" verb.
- [x] Headless-safe: steps 1, 7, 8 work without `ui` feature.
- [x] Frame discipline: panels read a change-gated view-model (§7).
- [x] TDD: tests first (`behavior_spec_test`, `checkpoint_click_test`,
      `run_tool` round-trip, waypoint-action parse).
- [x] Doc comments on all new items (§8).
- [x] No new `DomainKind` for checkpoints — reuse `Script`/`Usd` (per user).

## Implementation status (all 8 steps complete)

1. ✅ `AutopilotBehaviorSpec` component (`lunco-autopilot`) + tests.
2. ✅ `patrol.rhai` prelude + `to_json` bridge (`lunco-scripting`).
3. ✅ Checkpoint path-line gizmo (`lunco-sandbox-edit/src/checkpoint_gizmo.rs`).
4. ✅ Alt+LMB + right-click observers (`lunco-sandbox-edit/src/ui/checkpoint_click.rs`).
5. ✅ `CommandDeckPanel` (`lunco-sandbox-edit/src/ui/command_deck.rs`) — wired.
6. ✅ Avatar possession readout (`lunco-avatar/src/ui/mod.rs`) — producer + render.
7. ✅ `CaptureFromCamera` command + `take_photo` tool handler
      (`lunco-avatar/src/{screenshot,science}.rs`) + `photo()`/`photo_from()`
      rhai prelude (`science.rhai`, `control.rhai`).
8. ✅ `BehaviorSpec::RunTool` leaf + `ToolFired` event (one-shot latch) +
      `science.rhai` prelude.

## Architecture refinement (beyond the original plan)

The original step 7/8 design had rhai hand-composing behaviour trees and an
ad-hoc `ToolFired` consumer. Per the "core owns firing & cleaning, orchestration
in rhai" principle, the tool lifecycle moved into core — with the bevy-free /
bevy-aware split kept clean so the rhai-binding adapter stays slim:

- **`ToolFired` + `ToolInvocation`** moved to `lunco-core::tools` (shared
  vocabulary — handler crates don't depend on the autopilot emitter).
- **`lunco-tools` stays bevy-free** — it owns the discovery + script-binding
  `Tool` trait + global registry. No `execute()` here (that needs bevy).
- **`lunco-tools-bevy`** (new crate) owns the bevy-aware **`ExecutableTool`**
  supertrait + **`ClosureTool`**. It observes `ToolFired`, downcasts the
  registered tool to `ExecutableTool`, and runs it. **No JSON, no reflection** —
  a `ClosureTool`'s closure triggers its typed command directly via
  `DeferredWorld` (`world.trigger(CaptureFromCamera { target: vessel })`).
  `DeferredWorld` (not `&mut World`) because Bevy 0.19 forbids exclusive systems
  as observers. The closure IS the tool definition; adding an instrument is one
  closure, no per-instrument Rust struct. rhai only NAMES tools (`take_photo()`
  → a `run_tool` action value).
- **`PatrolWaypoint`** reshapes `Patrol` to carry per-waypoint `on_arrival`
  actions — the declarative home for "fire a tool at a patrol waypoint"
  (no rhai tree-composition). Backward-compatible: legacy bare-array
  `[[x,y,z],...]` JSON still parses.
- **`ClearPatrol`** command replaces the hand-built `Brake`-JSON dance in the
  Command Deck, the context menu, and the delete-last-waypoint path.