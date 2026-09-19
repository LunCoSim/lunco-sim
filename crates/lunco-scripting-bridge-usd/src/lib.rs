//! USD identity and document-generation projections for scripting backends.
//!
//! USD owns scene identity and document revisions. This adapter exposes those
//! facts to the language-neutral bridge without making its generic reflection,
//! command, or authority mechanism depend on the USD runtime packages.

use bevy::prelude::*;
use lunco_api::registry::ApiEntityRegistry;
use lunco_scripting_bridge_core::{resolve_entity, with_world};

/// Read the authoritative USD document generation without constructing the
/// full `InspectUsdDocument` snapshot.
///
/// A fixed-step policy may use this as its structural invalidation clock, then
/// perform its expensive topology read only when the generation changes. The
/// registry/document pair is the owner of this fact; this helper is only the
/// native language-neutral bridge to that owner.
pub fn usd_document_generation(doc_id: u64) -> Option<u64> {
    with_world(|world| {
        let registry = world.get_resource::<
            lunco_doc_bevy::DocumentRegistry<lunco_usd_document::document::UsdDocument>,
        >()?;
        registry
            .host(lunco_doc::DocumentId::new(doc_id))
            .map(|host| host.generation())
    })
    .flatten()
}

/// `find_path(path)` — first entity gid with the exact composed USD prim path,
/// or `-1`. Paths are the authored identity; this is intentionally separate
/// from `find`, whose name lookup is only a display convenience.
pub fn find_path(path: &str) -> i64 {
    with_world(|world| {
        let pairs = world.get_resource::<ApiEntityRegistry>()?.entities();
        pairs
            .into_iter()
            .find(|(_, entity)| {
                world
                    .get::<lunco_usd_bevy_scene::UsdPrimPath>(*entity)
                    .is_some_and(|prim| prim.path == path)
            })
            .map(|(id, _)| id.get() as i64)
    })
    .flatten()
    .unwrap_or(-1)
}

/// `usd_path(id)` — the exact composed USD path carried by an entity, or `()`.
/// This is the inverse of [`find_path`] and is the generic identity primitive
/// authored programs use to inspect their own scene-level ownership.
pub fn usd_path_of(gid: u64) -> Option<String> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        Some(
            world
                .get::<lunco_usd_bevy_scene::UsdPrimPath>(entity)?
                .path
                .clone(),
        )
    })
    .flatten()
}
