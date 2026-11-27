//! USD identity and document-generation projections for scripting backends.
//!
//! USD owns scene identity and document revisions. This adapter exposes those
//! facts to the language-neutral bridge without making its generic reflection,
//! command, or authority mechanism depend on the USD runtime packages.

use bevy::prelude::{Entity, World};
use lunco_api::registry::ApiEntityRegistry;
use lunco_scripting_bridge_core::{resolve_entity, with_world};
use lunco_usd_bevy_scene::prim_index::{MountedUsdScope, UsdPrimIndex};

/// Compare two USD attribute types using native Sdf semantic-role agreement.
pub fn usd_types_agree(source: &str, sink: &str) -> bool {
    openusd::sdf::ValueTypeName::from(source).agrees_with(&openusd::sdf::ValueTypeName::from(sink))
}

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

/// Stable scene and prim identity for progress owned by a mounted scenario host.
pub fn mounted_host_location(world: &World, gid: u64) -> Result<(String, String), String> {
    use lunco_usd_bevy_scene::{UsdPrimPath, scene_root_in_world};

    let host =
        resolve_entity(world, gid).ok_or("mission checkpoints require a live scenario host")?;
    let root = scene_root_in_world(world, host)
        .map_err(|err| format!("invalid mission host hierarchy: {err:?}"))?
        .ok_or("mission host is outside a mounted scene")?;
    if world
        .get_resource::<lunco_core::SceneMountState>()
        .and_then(|state| state.active_root())
        != Some(root)
        || lunco_usd_bevy_scene::is_preview_only_entity(world, host)
    {
        return Err("mission checkpoints require a host in the active scene".into());
    }
    let stage = world
        .get::<UsdPrimPath>(root)
        .ok_or("scene root has no USD stage")?;
    let path = stage
        .stage_handle
        .path()
        .ok_or("scene has no durable source path")?;
    let scene = lunco_assets_core::asset_path::source_uri(
        path,
        world.get_resource::<lunco_assets_core::TwinRoots>(),
    )
    .map_err(|err| format!("cannot identify mission scene: {err}"))?;
    let prim = world
        .get::<UsdPrimPath>(host)
        .ok_or("mission host has no authored prim path")?;
    if prim.stage_handle.id() != stage.stage_handle.id() {
        return Err("mission host needs a stable prim path in the mounted scene".into());
    }
    Ok((scene, prim.path.clone()))
}

/// Runtime identity must be unique, regardless of registry insertion order.
fn unique_registered_id(
    registry: &ApiEntityRegistry,
    candidates: impl Iterator<Item = Entity>,
) -> Option<i64> {
    let mut candidates = candidates;
    let target = candidates.next()?;
    if candidates.next().is_some() {
        bevy::log::warn!(
            "[script USD path] ambiguous live prim path; use an exact mounted-scene source"
        );
        return None;
    }
    registry.api_id_for(target).map(|id| id.get() as i64)
}

/// `find_path(path)` — unique live entity gid with the exact composed USD prim path,
/// or `-1`. Paths are the authored identity; this is intentionally separate
/// from `find`, whose name lookup is only a display convenience.
pub fn find_path(path: &str) -> i64 {
    with_world(|world| {
        let registry = world.get_resource::<ApiEntityRegistry>()?;
        let index = world.get_resource::<UsdPrimIndex>()?;
        unique_registered_id(
            registry,
            index
                .at_path(path)
                .filter(|entity| !lunco_usd_bevy_scene::is_preview_only_entity(world, *entity)),
        )
    })
    .flatten()
    .unwrap_or(-1)
}

/// `find_path_in_stage(source_gid, path)` — unique live entity gid in the
/// source entity's exact mounted scene, or `-1` if unavailable or ambiguous.
/// Preview projections are excluded; no document association is required.
pub fn find_path_in_stage(source_gid: u64, path: &str) -> i64 {
    with_world(|world| {
        let source = resolve_entity(world, source_gid)?;
        let scope = MountedUsdScope::from_source(world, source).ok()?;
        let api_entities = world.get_resource::<ApiEntityRegistry>()?;
        let index = world.get_resource::<UsdPrimIndex>()?;
        let entity = match index.unique_live_prim(world, scope.stage(), Some(scope.root()), path) {
            Ok(entity) => entity?,
            Err(error) => {
                bevy::log::warn!("[script USD path] {error}");
                return None;
            }
        };
        api_entities.api_id_for(entity).map(|id| id.get() as i64)
    })
    .flatten()
    .unwrap_or(-1)
}

/// `find_path_in_document(doc_id, path)` — unique live entity gid for `path` in the
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
        let entity = match index.unique_live_prim(world, stage, None, path) {
            Ok(entity) => entity?,
            Err(error) => {
                bevy::log::warn!("[script USD path] {error}");
                return None;
            }
        };
        entities.api_id_for(entity).map(|id| id.get() as i64)
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
