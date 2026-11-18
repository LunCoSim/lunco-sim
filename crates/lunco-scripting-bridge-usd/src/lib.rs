//! USD identity and document-generation projections for scripting backends.
//!
//! USD owns scene identity and document revisions. This adapter exposes those
//! facts to the language-neutral bridge without making its generic reflection,
//! command, or authority mechanism depend on the USD runtime packages.

use bevy::prelude::Entity;
use lunco_api::registry::ApiEntityRegistry;
use lunco_scripting_bridge_core::{resolve_entity, with_world};
use lunco_usd_bevy_scene::prim_index::UsdPrimIndex;

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

/// Multiple mounts can project the same path. The API identity determines
/// the selected entity regardless of Bevy entity slots or stage iteration order.
fn lowest_registered_id(
    registry: &ApiEntityRegistry,
    candidates: impl Iterator<Item = Entity>,
) -> Option<i64> {
    candidates
        .filter_map(|entity| registry.api_id_for(entity))
        .map(|id| id.get())
        .min()
        .map(|id| id as i64)
}

/// `find_path(path)` — first entity gid with the exact composed USD prim path,
/// or `-1`. Paths are the authored identity; this is intentionally separate
/// from `find`, whose name lookup is only a display convenience.
pub fn find_path(path: &str) -> i64 {
    with_world(|world| {
        let registry = world.get_resource::<ApiEntityRegistry>()?;
        let index = world.get_resource::<UsdPrimIndex>()?;
        lowest_registered_id(registry, index.at_path(path))
    })
    .flatten()
    .unwrap_or(-1)
}

/// `find_path_in_stage(source_gid, path)` — first live entity gid for `path` in
/// the exact mounted scene containing `source_gid`, or `-1`. Multiple mounts
/// can share one stage asset, so matching stays within the source's scene root.
/// Preview projections are excluded. This identity boundary is available
/// before the live stage has a document association.
pub fn find_path_in_stage(source_gid: u64, path: &str) -> i64 {
    with_world(|world| {
        let source = resolve_entity(world, source_gid)?;
        let stage = world
            .get::<lunco_usd_bevy_scene::UsdPrimPath>(source)?
            .stage_handle
            .id();
        let source_root = lunco_usd_bevy_scene::scene_root_in_world(world, source).ok()??;
        let api_entities = world.get_resource::<ApiEntityRegistry>()?;
        let index = world.get_resource::<UsdPrimIndex>()?;
        let candidates = index
            .in_stage(stage, path)
            .iter()
            .copied()
            .filter(|entity| {
                !lunco_usd_bevy_scene::is_preview_only_entity(world, *entity)
                    && lunco_usd_bevy_scene::scene_root_in_world(world, *entity)
                        .ok()
                        .flatten()
                        == Some(source_root)
            });
        lowest_registered_id(api_entities, candidates)
    })
    .flatten()
    .unwrap_or(-1)
}

/// `find_path_in_document(doc_id, path)` — first entity gid for `path` in the
/// live stage owned by `doc_id`, or `-1` when that prim has no live projection.
/// USD paths are only unique inside their composed stage, so editing a
/// document-backed prim must include its document identity.
pub fn find_path_in_document(doc_id: u64, path: &str) -> i64 {
    with_world(|world| {
        let doc = lunco_doc::DocumentId::new(doc_id);
        let backed = world.get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()?;
        let stage = lunco_usd_bevy_twin::stage_asset_for_document(backed, doc)?;
        let entities = world.get_resource::<ApiEntityRegistry>()?;
        let index = world.get_resource::<UsdPrimIndex>()?;
        let candidates = index
            .in_stage(stage, path)
            .iter()
            .copied()
            .filter(|entity| !lunco_usd_bevy_scene::is_preview_only_entity(world, *entity));
        lowest_registered_id(entities, candidates)
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
