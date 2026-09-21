//! Render-only marker copies for body-fixed entities in the detached sky.
//!
//! Functional station entities stay under the causal body grid. Their visible
//! screen markers also get a copy under the body's CelestialTime presentation
//! grid, so the marker remains on the moving globe without changing link or
//! physics coordinates.

use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_celestial::geo::GeodeticAnchor;
use lunco_celestial::ReferenceFrame;
use lunco_celestial_spatial_core::ReferenceFrameIndex;
use lunco_materials::ShaderLook;
use lunco_render::{PbrLook, ScreenConstantMarker, ScreenConstantMarkerVisibility};
use std::collections::{HashMap, HashSet};

#[derive(Component)]
pub(crate) struct PresentationMarkerReplica {
    source: Entity,
}

type DirtyMarker = Or<(
    Added<ScreenConstantMarker>,
    Changed<ScreenConstantMarker>,
    Changed<Transform>,
    Changed<ChildOf>,
    Added<Mesh3d>,
    Changed<Mesh3d>,
    Added<PbrLook>,
    Changed<PbrLook>,
    Added<ShaderLook>,
    Changed<ShaderLook>,
)>;

/// Wake the synchronizer for authored edits, hierarchy changes, and stale
/// replicas. Screen-marker distance scaling can also dirty its source transform;
/// the synchronizer compares only pose fields before writing BigSpace state.
pub(crate) fn presentation_markers_need_sync(
    sources: Query<
        (),
        (
            With<ScreenConstantMarker>,
            Without<PresentationMarkerReplica>,
            DirtyMarker,
        ),
    >,
    anchors: Query<
        (),
        Or<(
            Added<GeodeticAnchor>,
            Changed<GeodeticAnchor>,
            Changed<Transform>,
            Changed<ChildOf>,
        )>,
    >,
    grids: Query<(), Added<crate::CelestialPresentationGrid>>,
    replicas: Query<&PresentationMarkerReplica>,
    all_sources: Query<
        Entity,
        (
            With<ScreenConstantMarker>,
            Without<PresentationMarkerReplica>,
        ),
    >,
    mut removed_markers: RemovedComponents<ScreenConstantMarker>,
    mut removed_meshes: RemovedComponents<Mesh3d>,
    mut removed_pbr_looks: RemovedComponents<PbrLook>,
    mut removed_shader_looks: RemovedComponents<ShaderLook>,
) -> bool {
    let marker_removed = removed_markers.read().count() > 0;
    let mesh_removed = removed_meshes.read().count() > 0;
    let pbr_look_removed = removed_pbr_looks.read().count() > 0;
    let shader_look_removed = removed_shader_looks.read().count() > 0;
    let source_component_removed =
        marker_removed || mesh_removed || pbr_look_removed || shader_look_removed;
    source_component_removed
        || !sources.is_empty()
        || !anchors.is_empty()
        || !grids.is_empty()
        || replicas
            .iter()
            .any(|replica| all_sources.get(replica.source).is_err())
}

/// Mirror screen markers below a geodetic anchor into the matching detached
/// body-fixed presentation grid.
#[allow(clippy::type_complexity)]
pub(crate) fn sync_presentation_markers(
    frame_index: Res<ReferenceFrameIndex>,
    sources: Query<
        (
            Entity,
            Ref<ScreenConstantMarker>,
            Option<Ref<Mesh3d>>,
            Option<Ref<PbrLook>>,
            Option<Ref<ShaderLook>>,
            Option<&ScreenConstantMarkerVisibility>,
        ),
        Without<PresentationMarkerReplica>,
    >,
    anchors: Query<(Entity, &GeodeticAnchor)>,
    parents: Query<&ChildOf>,
    grids: Query<&Grid>,
    presentation_grids: Query<(Entity, &crate::CelestialPresentationGrid), With<Grid>>,
    spatial: Query<(Option<&CellCoord>, &Transform), Without<PresentationMarkerReplica>>,
    mut replicas: Query<
        (
            Entity,
            &PresentationMarkerReplica,
            &mut CellCoord,
            &mut Transform,
            &mut ScreenConstantMarker,
            Option<&mut Mesh3d>,
            Option<&mut PbrLook>,
            Option<&mut ShaderLook>,
            &mut ScreenConstantMarkerVisibility,
            &ChildOf,
        ),
        With<PresentationMarkerReplica>,
    >,
    mut commands: Commands,
) {
    let existing: HashMap<_, _> = replicas
        .iter_mut()
        .map(|(entity, replica, ..)| (replica.source, entity))
        .collect();
    let mut live_sources = HashSet::new();

    for (source, marker, mesh, pbr_look, shader_look, marker_visibility) in &sources {
        let Some(anchor) = geodetic_anchor_ancestor(source, &anchors, &parents) else {
            if marker_visibility.is_some() {
                commands
                    .entity(source)
                    .remove::<ScreenConstantMarkerVisibility>();
            }
            continue;
        };
        let mesh_changed = mesh.as_ref().is_some_and(|mesh| mesh.is_changed());
        let mesh = mesh.as_ref().map(|mesh| (**mesh).clone());
        let pbr_changed = pbr_look.as_ref().is_some_and(|look| look.is_changed());
        let pbr_look = pbr_look.as_ref().map(|look| (**look).clone());
        let shader_changed = shader_look.as_ref().is_some_and(|look| look.is_changed());
        let shader_look = shader_look.as_ref().map(|look| (**look).clone());

        let Some(mesh) = mesh else {
            warn_once!("[celestial] a geodetic screen marker has no Mesh3d and cannot be mirrored");
            if marker_visibility.is_some() {
                commands
                    .entity(source)
                    .remove::<ScreenConstantMarkerVisibility>();
            }
            continue;
        };
        if pbr_look.is_none() && shader_look.is_none() {
            warn_once!(
                "[celestial] a geodetic screen marker has no render look and cannot be mirrored"
            );
            if marker_visibility.is_some() {
                commands
                    .entity(source)
                    .remove::<ScreenConstantMarkerVisibility>();
            }
            continue;
        }
        live_sources.insert(source);

        let Some(physical_grid) =
            frame_index.resolve(ReferenceFrame::BodyFixed { body: anchor.body })
        else {
            if marker_visibility.is_some() {
                commands
                    .entity(source)
                    .remove::<ScreenConstantMarkerVisibility>();
            }
            continue;
        };
        let Some(presentation_grid) = presentation_grids.iter().find_map(|(entity, frame)| {
            (frame.body == anchor.body && frame.body_fixed).then_some(entity)
        }) else {
            if marker_visibility.is_some() {
                commands
                    .entity(source)
                    .remove::<ScreenConstantMarkerVisibility>();
            }
            continue;
        };

        let Some((body_fixed_position, body_fixed_rotation)) =
            lunco_spatial::coords::pose_in_grid(source, physical_grid, &parents, &grids, &spatial)
        else {
            // Grid migration and scene projection are deferred. The source or
            // anchor change that follows them re-opens this synchronizer.
            continue;
        };
        let Ok(presentation_grid_data) = grids.get(presentation_grid) else {
            continue;
        };
        let (cell, translation) = presentation_grid_data.translation_to_grid(body_fixed_position);
        let rotation = body_fixed_rotation.as_quat();
        if marker_visibility.is_none() {
            commands
                .entity(source)
                .try_insert(ScreenConstantMarkerVisibility::default());
        }

        if let Some(replica_entity) = existing.get(&source).copied() {
            let Ok((
                _,
                _,
                mut current_cell,
                mut current_transform,
                mut current_marker,
                mut current_mesh,
                mut current_pbr,
                mut current_shader,
                _current_visibility,
                current_parent,
            )) = replicas.get_mut(replica_entity)
            else {
                continue;
            };
            if *current_cell != cell {
                *current_cell = cell;
            }
            if current_transform.translation != translation {
                current_transform.translation = translation;
            }
            if current_transform.rotation != rotation {
                current_transform.rotation = rotation;
            }
            if *current_marker != *marker {
                *current_marker = *marker;
            }
            if current_parent.parent() != presentation_grid {
                commands
                    .entity(replica_entity)
                    .insert(ChildOf(presentation_grid));
            }
            if mesh_changed {
                if let Some(current_mesh) = current_mesh.as_mut() {
                    **current_mesh = mesh.clone();
                } else {
                    commands.entity(replica_entity).insert(mesh.clone());
                }
            }
            if pbr_changed {
                if let Some(current_pbr) = current_pbr.as_mut() {
                    if let Some(look) = pbr_look.as_ref() {
                        **current_pbr = look.clone();
                    }
                } else if let Some(look) = pbr_look.as_ref() {
                    commands.entity(replica_entity).insert(look.clone());
                }
            }
            if pbr_look.is_none() && current_pbr.is_some() {
                commands.entity(replica_entity).remove::<PbrLook>();
            }
            if shader_changed {
                if let Some(current_shader) = current_shader.as_mut() {
                    if let Some(look) = shader_look.as_ref() {
                        **current_shader = look.clone();
                    }
                } else if let Some(look) = shader_look.as_ref() {
                    commands.entity(replica_entity).insert(look.clone());
                }
            }
            if shader_look.is_none() && current_shader.is_some() {
                commands.entity(replica_entity).remove::<ShaderLook>();
            }
        } else {
            let mut bundle = commands.spawn((
                mesh,
                *marker,
                cell,
                Transform {
                    translation,
                    rotation,
                    ..default()
                },
                GlobalTransform::default(),
                Visibility::Inherited,
                InheritedVisibility::default(),
                Name::new(format!("Celestial marker {source:?}")),
                PresentationMarkerReplica { source },
                ScreenConstantMarkerVisibility::default(),
                ChildOf(presentation_grid),
            ));
            if let Some(look) = pbr_look {
                bundle.insert(look);
            }
            if let Some(look) = shader_look {
                bundle.insert(look);
            }
        }
    }

    for (entity, replica, ..) in replicas.iter_mut() {
        if !live_sources.contains(&replica.source) {
            commands.entity(entity).despawn();
        }
    }
}

/// Keep the causal marker on its own surface and select its detached copy in
/// orbit or from another body's surface. The shared marker scaler still applies
/// each marker's authored visibility distance.
pub(crate) fn update_presentation_marker_visibility(
    surface_poses: lunco_celestial_spatial_core::SurfacePoseQuery,
    cameras: Query<(Entity, &Camera), With<lunco_render::SceneCamera>>,
    anchors: Query<(Entity, &GeodeticAnchor)>,
    parents: Query<&ChildOf>,
    mut sources: Query<
        (Entity, &mut ScreenConstantMarkerVisibility),
        Without<PresentationMarkerReplica>,
    >,
    mut replicas: Query<
        (
            &PresentationMarkerReplica,
            &mut ScreenConstantMarkerVisibility,
        ),
        With<PresentationMarkerReplica>,
    >,
) {
    let mut active_cameras = cameras.iter().filter(|(_, camera)| camera.is_active);
    let active_camera = active_cameras.next();
    let camera_is_unambiguous = active_camera.is_some() && active_cameras.next().is_none();
    let camera_body = active_camera
        .filter(|_| camera_is_unambiguous)
        .and_then(|(camera, _)| surface_poses.get(camera).map(|pose| pose.body));
    let mut replica_sources = HashSet::new();

    for (replica, mut visibility) in &mut replicas {
        replica_sources.insert(replica.source);
        let source_body =
            geodetic_anchor_ancestor(replica.source, &anchors, &parents).map(|anchor| anchor.body);
        let visible = camera_is_unambiguous && source_body != camera_body;
        if visibility.visible != visible {
            visibility.visible = visible;
        }
    }
    for (source, mut visibility) in &mut sources {
        let source_body =
            geodetic_anchor_ancestor(source, &anchors, &parents).map(|anchor| anchor.body);
        let visible = camera_is_unambiguous
            && replica_sources.contains(&source)
            && source_body.is_some()
            && source_body == camera_body;
        if visibility.visible != visible {
            visibility.visible = visible;
        }
    }
}

fn geodetic_anchor_ancestor(
    source: Entity,
    anchors: &Query<(Entity, &GeodeticAnchor)>,
    parents: &Query<&ChildOf>,
) -> Option<GeodeticAnchor> {
    let mut current = source;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            warn_once!("[celestial] cycle in geodetic marker ancestry");
            return None;
        }
        if let Ok((_, anchor)) = anchors.get(current) {
            return Some(*anchor);
        }
        current = parents.get(current).ok()?.parent();
    }
}
