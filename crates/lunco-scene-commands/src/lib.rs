//! # LunCoSim Scene Commands
//!
//! Scene commands route interactive actions to runtime owners. Explicit persistent
//! edits commit through the owning USD document and its journal.
//!
//! - [`commands`] — scene-command observers and
//!   [`commands::SpawnCommandPlugin`], the mutation plugin used by headless
//!   and interactive composition roots. Shared payload definitions live in
//!   `lunco-scene-command-contracts` so producers do not depend on handlers.
//! - `lunco-scene-camera` — camera framing commands and their active-frame
//!   focus transaction, installed by each application composition root that
//!   exposes camera commands.
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
//! headless server links it **without** linking either editor package. Editor
//! producers that only emit scene edits depend on the focused command-contract
//! package; application composition depends on this crate to install handlers.
//!
//! ## Adding New Spawn Types
//!
//! Author a USD asset with `lunco:spawnable = true` and place it under the
//! project asset roots. The catalog discovers it asynchronously and exposes it
//! through `ListSpawnCatalog`; no Rust catalog entry is required. If two sources
//! share a file stem, the catalog keeps both and suffixes the later ID with its
//! source path.

pub mod commands;
mod environment_light;
pub mod scene_view;

mod scene_participation;
