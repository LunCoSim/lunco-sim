//! Shared helpers for production USD integration tests.

use avian3d::prelude::Collider;
use bevy::prelude::{App, Children, Entity, Name, Or, With};
use lunco_usd_bevy::{
    CanonicalStage, CanonicalStages, StageRecipe, UsdAwaitingStage, UsdStageAsset,
    UsdVisualProjectionQueued,
};
use std::collections::HashMap;
use std::path::Path;

/// Extract cuboid half-extents from either a plain collider or a compound made
/// of cuboid collision children. The simulation tests assert the authored
/// chassis shape, while the production bridge is free to compose several
/// collision children onto one rigid body.
#[allow(dead_code)]
pub(crate) fn cuboid_half_extents(col: &Collider) -> Vec<[f32; 3]> {
    let shape = col.shape();
    if let Some(c) = shape.as_cuboid() {
        return vec![
            [
                c.half_extents.x as f32,
                c.half_extents.y as f32,
                c.half_extents.z as f32,
            ],
        ];
    }
    if let Some(compound) = shape.as_compound() {
        return compound
            .shapes()
            .iter()
            .filter_map(|(_, shape)| shape.as_cuboid())
            .map(|c| {
                [
                    c.half_extents.x as f32,
                    c.half_extents.y as f32,
                    c.half_extents.z as f32,
                ]
            })
            .collect();
    }
    panic!(
        "collider is neither a cuboid nor a compound-of-cuboid: {:?}",
        shape.shape_type()
    );
}

/// Return the Chassis child carrying a rover's visible body mesh after the
/// Xform-root projection. Both simulation pipeline tests assert this same
/// production hierarchy.
#[allow(dead_code)]
pub(crate) fn chassis_child(app: &App, rover: Entity, label: impl std::fmt::Display) -> Entity {
    let kids = app
        .world()
        .get::<Children>(rover)
        .unwrap_or_else(|| panic!("{label}: rover missing Children"));
    kids.iter()
        .find_map(|c| {
            let c = *c;
            app.world().get::<Name>(c).and_then(|n| {
                n.as_str().contains("Chassis").then_some(c)
            })
        })
        .unwrap_or_else(|| panic!("{label}: rover has no Chassis child"))
}

/// Build the same prepared layer-closure asset used by the production USD
/// loader, while also installing its explicit live authoring stage for a
/// synchronous integration harness. Keeping both representations in the
/// fixture prevents tests from silently bypassing the prepared initial
/// projection contract.
#[allow(dead_code)] // Each integration target imports this shared module independently.
pub(crate) fn add_prepared_canonical_from_file(
    app: &mut App,
    file_path: &Path,
) -> bevy::asset::Handle<UsdStageAsset> {
    let assets_root = lunco_assets::shipped_asset_root(file_path);
    let root_id = match assets_root.and_then(|root| file_path.strip_prefix(root).ok()) {
        Some(relative) => {
            lunco_assets::engine_asset_uri(&lunco_assets::asset_path::slashed(relative))
        }
        None => lunco_assets::asset_path::canonicalize_root(&file_path.to_string_lossy()),
    };
    let mut bytes = HashMap::from([(
        root_id.clone(),
        lunco_assets::read_asset_file_bytes(file_path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", file_path.display())),
    )]);
    let mut queue = vec![root_id.clone()];
    while let Some(id) = queue.pop() {
        let raw = bytes.get(&id).expect("queued layer is present").clone();
        for child_id in lunco_usd_compose::child_layer_ids(&id, &raw)
            .unwrap_or_else(|error| panic!("cannot inspect USD dependencies in {id}: {error}"))
        {
            if bytes.contains_key(&child_id) {
                continue;
            }
            let child = lunco_assets::read_asset_bytes_with_twin_root(&child_id, assets_root, None)
                .unwrap_or_else(|error| panic!("cannot read USD dependency {child_id}: {error}"));
            bytes.insert(child_id.clone(), child);
            queue.push(child_id);
        }
    }
    let recipe = StageRecipe { root_id, bytes };
    let handle = app
        .world_mut()
        .resource_mut::<bevy::asset::Assets<UsdStageAsset>>()
        .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare USD asset"));
    let canonical = CanonicalStage::from_recipe(&recipe).expect("build live USD stage");
    app.world_mut()
        .get_non_send_mut::<CanonicalStages>()
        .expect("CanonicalStages resource (UsdBevyPlugin)")
        .insert(handle.id(), canonical);
    handle
}

/// Advance the production projection pipeline until no USD prim is waiting for
/// a stage or visual projection, including the dependent observer frame.
pub(crate) fn settle_visual_projection(app: &mut App) {
    let mut empty_frames = 0;
    for _ in 0..1000 {
        app.update();
        let pending = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<(), Or<(
                With<UsdVisualProjectionQueued>,
                With<UsdAwaitingStage>,
            )>>();
            query.iter(world).next().is_some()
        };
        if pending {
            empty_frames = 0;
        } else {
            empty_frames += 1;
            if empty_frames == 2 {
                return;
            }
        }
    }
    panic!("USD visual projection did not settle within 1000 frames");
}
