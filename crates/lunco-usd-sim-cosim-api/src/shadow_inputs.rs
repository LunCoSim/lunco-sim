//! Read-only projection of Bevy's native main-world directional-shadow facts.
//!
//! Exact render-space f32 bits distinguish changed inputs from change ticks.
//! Shared fingerprints keep the response compact; raw poses use bounded pages.
//! These facts omit render-world allocation, material, pipeline and deformation
//! state, so an unchanged snapshot alone cannot authorize shadow-content reuse.

use bevy::{
    asset::AssetId,
    camera::visibility::CascadesVisibleEntities,
    ecs::query::QueryState,
    light::{Cascades, cascade::Cascade},
    prelude::*,
};
use lunco_api_core::{ApiValue, api_value};
use lunco_hash::Fnv1a;
use lunco_usd_bevy_scene::UsdPrimPath;

/// The inspection protocol bounds raw poses per map, independently of scene size.
const MAX_CASTER_PAGE: usize = 64;

/// Requested raw-pose page; zero limit requests fingerprints without raw poses.
pub(super) struct CasterPage {
    offset: usize,
    limit: usize,
}

/// Validate bounded raw-pose inspection at the query's external value boundary.
pub(super) fn page_options(
    params: &ApiValue,
    included: bool,
) -> Result<CasterPage, lunco_api::ApiQueryError> {
    let read = |name: &str| -> Result<usize, lunco_api::ApiQueryError> {
        let Some(value) = params.get(name) else {
            return Ok(0);
        };
        if !included {
            return Err(lunco_api::ApiQueryError::new(
                lunco_api_core::ApiErrorCode::DeserializationError,
                format!("SceneCameraAudit: {name} requires shadow_inputs: true"),
            ));
        }
        crate::api_u64(value)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                lunco_api::ApiQueryError::new(
                    lunco_api_core::ApiErrorCode::DeserializationError,
                    format!("SceneCameraAudit: {name} must be a non-negative integer"),
                )
            })
    };
    let page = CasterPage {
        offset: read("shadow_caster_offset")?,
        limit: read("shadow_caster_limit")?,
    };
    if page.limit > MAX_CASTER_PAGE {
        return Err(lunco_api::ApiQueryError::new(
            lunco_api_core::ApiErrorCode::DeserializationError,
            format!("SceneCameraAudit: shadow_caster_limit must be at most {MAX_CASTER_PAGE}"),
        ));
    }
    Ok(page)
}

/// Preserve the native matrix representation without precision conversion.
fn matrix_bits(matrix: Mat4) -> [u32; 16] {
    matrix.to_cols_array().map(f32::to_bits)
}

/// Describe one native visibility candidate, retaining unavailable fields.
fn caster_snapshot(world: &World, entity: Entity) -> ApiValue {
    let transform = world
        .get::<GlobalTransform>(entity)
        .map(GlobalTransform::to_matrix);
    api_value!({
        "entity": entity.to_bits(),
        "usd_path": world.get::<UsdPrimPath>(entity).map(|path| path.path.as_str()),
        "world_from_local_bits": transform.map(matrix_bits),
        "world_from_local_finite": transform.map(|matrix| matrix.is_finite()),
        "mesh_asset": world.get::<Mesh3d>(entity).map(|mesh| format!("{:?}", mesh.0.id())),
    })
}

/// Fold native membership, pose bits and typed mesh identities without serialization.
fn caster_fingerprints(world: &World, entities: &[Entity]) -> ApiValue {
    let mut membership = Fnv1a::new();
    let mut poses = Fnv1a::new();
    let mut meshes = Fnv1a::new();
    membership.write_u64(entities.len() as u64);
    poses.write_u64(entities.len() as u64);
    meshes.write_u64(entities.len() as u64);
    let mut poses_available = true;
    let mut poses_finite = true;
    let mut meshes_available = true;
    for &entity in entities {
        membership.write_u64(entity.to_bits());
        poses.write_u64(entity.to_bits());
        meshes.write_u64(entity.to_bits());
        if let Some(transform) = world.get::<GlobalTransform>(entity) {
            let matrix = transform.to_matrix();
            poses.write_u64(1);
            for bits in matrix_bits(matrix) {
                poses.write_u64(u64::from(bits));
            }
            poses_finite &= matrix.is_finite();
        } else {
            poses.write_u64(0);
            poses_available = false;
            poses_finite = false;
        }
        match world.get::<Mesh3d>(entity).map(|mesh| mesh.0.id()) {
            Some(AssetId::Index { index, .. }) => {
                meshes.write_u64(1).write_u64(index.to_bits());
            }
            Some(AssetId::Uuid { uuid }) => {
                let (high, low) = uuid.as_u64_pair();
                meshes.write_u64(2).write_u64(high).write_u64(low);
            }
            None => {
                meshes.write_u64(0);
                meshes_available = false;
            }
        }
    }
    api_value!({
        "algorithm": "fnv1a64-words-v1",
        "membership": format!("{:016x}", membership.finish()),
        "poses": format!("{:016x}", poses.finish()),
        "meshes": format!("{:016x}", meshes.finish()),
        "poses_available": poses_available,
        "poses_finite": poses_finite,
        "meshes_available": meshes_available,
    })
}

/// Project one native map and an explicitly bounded page of its candidates.
fn map_snapshot(
    world: &World,
    camera: Entity,
    index: usize,
    cascade: &Cascade,
    visible: Option<&CascadesVisibleEntities>,
    page: &CasterPage,
) -> ApiValue {
    let entities = visible
        .and_then(|visible| visible.entities.get(&camera))
        .and_then(|cascades| cascades.get(index))
        .map(|visible| {
            let mut entities = visible.entities.clone();
            entities.sort_by_key(|entity| entity.to_bits());
            entities
        });
    let fingerprints = entities
        .as_ref()
        .map(|entities| caster_fingerprints(world, entities));
    let casters = entities
        .as_ref()
        .filter(|_| page.limit > 0)
        .map(|entities| {
            entities
                .iter()
                .skip(page.offset)
                .take(page.limit)
                .map(|&entity| caster_snapshot(world, entity))
                .collect::<Vec<_>>()
        });
    api_value!({
        "camera": camera.to_bits(),
        "camera_name": world.get::<Name>(camera).map(|name| name.as_str()),
        "camera_3d": world.get::<Camera3d>(camera).is_some(),
        "camera_active": world.get::<Camera>(camera).map(|camera| camera.is_active),
        "cascade": index,
        "world_from_cascade_bits": matrix_bits(cascade.world_from_cascade),
        "clip_from_cascade_bits": matrix_bits(cascade.clip_from_cascade),
        "clip_from_world_bits": matrix_bits(cascade.clip_from_world),
        "texel_size_bits": cascade.texel_size.to_bits(),
        "matrices_finite": cascade.world_from_cascade.is_finite()
            && cascade.clip_from_cascade.is_finite() && cascade.clip_from_world.is_finite()
            && cascade.texel_size.is_finite(),
        "caster_total": entities.as_ref().map(|entities| entities.len()),
        "caster_fingerprints": fingerprints,
        "caster_offset": page.offset,
        "caster_limit": page.limit,
        "caster_page_complete": entities.as_ref().map(|entities|
            page.limit > 0 && page.offset == 0 && entities.len() <= page.limit),
        "caster_candidates": casters,
    })
}

/// Inspect native cascades only on an explicit request; never alter their owner.
pub(super) fn snapshot(world: &World, page: &CasterPage) -> ApiValue {
    let host_frame = world
        .get_resource::<bevy::diagnostic::FrameCount>()
        .map(|frame| frame.0);
    let Some(mut query) = QueryState::<(
        Entity,
        Option<&Name>,
        Option<&UsdPrimPath>,
        &DirectionalLight,
        &Cascades,
        Option<&CascadesVisibleEntities>,
    )>::try_new(world) else {
        return api_value!({ "host_frame": host_frame, "native_types_registered": false, "lights": ApiValue::Unit });
    };
    let mut lights: Vec<_> = query.iter(world).collect();
    lights.sort_by_key(|(entity, ..)| entity.to_bits());
    let lights: Vec<_> = lights
        .into_iter()
        .map(|(entity, name, path, light, cascades, visible)| {
            let mut views: Vec<_> = cascades.cascades.iter().collect();
            views.sort_by_key(|(camera, _)| camera.to_bits());
            let mut maps = Vec::new();
            for (camera, camera_cascades) in views {
                for (index, cascade) in camera_cascades.iter().enumerate() {
                    maps.push(map_snapshot(world, *camera, index, cascade, visible, page));
                }
            }
            api_value!({
                "entity": entity.to_bits(),
                "name": name.map(|name| name.as_str()),
                "usd_path": path.map(|path| path.path.as_str()),
                "shadow_maps_enabled": light.shadow_maps_enabled,
                "maps": maps,
            })
        })
        .collect();
    api_value!({ "host_frame": host_frame, "native_types_registered": true, "lights": lights })
}
