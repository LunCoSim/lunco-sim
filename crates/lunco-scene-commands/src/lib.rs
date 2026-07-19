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
//!   `DeleteEntity`, `SetObjectProperty`, `SetShaderSource`, …) plus
//!   [`commands::SpawnCommandPlugin`], the one plugin a headless server adds.
//! - [`catalog`] — the spawn catalog (what can be spawned) and the shader catalog,
//!   scanned from the engine's `*.usda` library.
//! - [`spawn_meta`] — the ONE parser for `lunco:spawnable` / `lunco:spawnLift` /
//!   `lunco:description`, shared verbatim with `build.rs`, which bakes the same table
//!   for the filesystem-less web build.
//! - [`shader_doc`] — shaders as a journaled, live-editable document domain.
//! - [`doc_resolve`] — which document backs this entity, and where its look lives.
//!
//! ## Render-free, UI-free
//!
//! This crate names no material type and no egui/winit/picking/gizmo crate, so the
//! headless server links it **without** linking the editor (`lunco-sandbox-edit`,
//! which now depends on *this* crate rather than containing it). The one GUI-shaped
//! concession is the optional `ui` feature, which only re-enables
//! `doc_resolve`'s fallback to the viewport's active document — see that module.
//!
//! ## Adding New Spawn Types
//!
//! Add entries to `SpawnCatalog::default()` in `catalog.rs`:
//!
//! ```ignore
//! catalog.add(SpawnableEntry {
//!     id: "my_rover".into(),
//!     display_name: "My Rover".into(),
//!     category: SpawnCategory::Rover,
//!     source: SpawnSource::UsdFile("vessels/rovers/my_rover.usda".into()),
//!     default_transform: Transform::default(),
//! });
//! ```

pub mod catalog;
pub mod commands;
/// `QueryEntity` — the READ side of the scene verbs, reporting the same
/// grid-absolute frame [`commands::MoveEntity`] accepts.
pub mod entity_query;
/// `QueryUsdPrim` — the AUTHORED read: composed USD attributes off the live
/// stage, for asset invariants that scripts (not just Rust) can check.
pub mod usd_prim_query;
/// Headless-safe: resolve an entity's backing USD document + its bound shader prim.
/// Shared by `commands` (the authoring tier) and the editor's Inspector panel — it
/// lived in the panel, which is what broke the `--no-ui` server build (`commands`
/// reached into `crate::ui` for it).
pub mod doc_resolve;
/// Shaders as a journaled, synced, live-editable domain (WGSL twin of rhai's
/// `ScriptDocument`) — edits record to the Twin journal (`DomainKind::Shader`).
pub mod shader_doc;
pub mod spawn_meta;

use bevy::prelude::*;

/// Tracks which entities are currently selected.
///
/// Lives here, not in the editor: `commands` both mutates it (a deleted entity leaves
/// the selection) and `init_resource`s it, so it is part of the command layer's own
/// state. `lunco-sandbox-edit` re-exports it for its panels.
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
