//! # LunCoSim Scene Commands
//!
//! The **scene/document command layer**: every runtime mutation of the scene —
//! spawn, move, delete, set-property, shader edits — authored as journaled USD ops
//! on the open document's runtime layer.
//!
//! One path, four callers. An edit made by a rhai script, by the HTTP API, by a peer
//! arriving over the wire, or by a human dragging a gizmo in the editor all funnel
//! through the same commands here, so they are all persisted, journaled, undoable and
//! replicated identically. An edit that does not go through this layer escapes
//! save/journal/undo/network.
//!
//! - [`commands`] — the command set itself (`SpawnEntity`, `MoveEntity`,
//!   `TransformEntity`, `DeleteEntity`, and `SetUsdConnection`, …) plus
//!   [`commands::SpawnCommandPlugin`], the one plugin a headless server adds.
//! - `lunco-scene-catalog` — the spawn and source catalogs, asynchronous asset
//!   discovery, and the generic USD spawn constructor.
//! - `lunco-scene-queries` — shared read-only entity and composed-USD query
//!   providers for Rhai, HTTP, MCP, and headless hosts.
//! - `lunco-scene-authoring` — document ownership, property persistence, and
//!   live shader authoring as a journaled domain.
//!
//! ## Render-free, UI-free
//!
//! This crate names no material type and no egui/winit/picking/gizmo crate, so the
//! headless server links it **without** linking either editor package. The editor
//! packages depend on this crate rather than containing the command layer. The optional
//! `ui` feature exposes shared USD UI types through the editor dependency graph;
//! it does not change document resolution or authoring semantics.
//!
//! ## Adding New Spawn Types
//!
//! Author a USD asset with `lunco:spawnable = true` and place it under the
//! project asset roots. The catalog discovers it asynchronously and exposes it
//! through `ListSpawnCatalog`; no Rust catalog entry is required. If two sources
//! share a file stem, the catalog keeps both and suffixes the later ID with its
//! source path.

pub mod commands;
/// Static discovery of authored scene tests and their headless/graphics kind.
/// The scene supplies the USD program binding; the Rhai test source supplies
/// the execution domain.
#[cfg(not(target_arch = "wasm32"))]
pub mod test_discovery;

use bevy::prelude::*;

/// Tracks which entities are currently selected.
///
/// Lives here, not in the editor: `commands` both mutates it (a deleted entity leaves
/// the selection) and `init_resource`s it, so it is part of the command layer's own
/// state. The editor consumes this resource directly.
#[derive(Resource, Default, Clone)]
pub struct SelectedEntities {
    /// The selected entities. The last one added is the "primary" selection.
    pub entities: Vec<Entity>,
}

impl SelectedEntities {
    /// Returns the primary selected entity, if any.
    pub fn primary(&self) -> Option<Entity> {
        self.entities.last().copied()
    }
}

/// Mirror the selection into [`lunco_signal::TelemetryFocus`] — the render-free
/// "what is the user looking at" resource every telemetry surface reads to narrow
/// itself ("the selected vessel's channels", not the whole sim's).
///
/// It lives HERE, beside [`SelectedEntities`] itself, rather than in the editor:
/// selection is command-layer state, and any host that links the command layer —
/// the sandbox, the Modelica workbench, a headless server driven by
/// `SelectEntity` over HTTP — should get the same scoping without re-implementing
/// this. Every host that links the command layer therefore gets the same
/// selection scoping without a UI dependency.
///
/// Change-driven: writes only when the selection actually moved AND the mirror
/// would differ, so the resource's own change tick stays meaningful downstream.
pub fn mirror_selection_to_telemetry_focus(
    selected: Res<SelectedEntities>,
    mut focus: ResMut<lunco_signal::TelemetryFocus>,
) {
    if !selected.is_changed() {
        return;
    }
    if focus.roots != selected.entities {
        focus.roots.clone_from(&selected.entities);
    }
}
