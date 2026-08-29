//! Bounded, terrain-conforming motion trails for topology-derived vehicles.
//!
//! A trail is presentation history, not a second vehicle state. The vehicle's
//! Avian [`Position`] is the only motion source; the history is stored in the
//! active physics/grid frame and is never inferred from render transforms or
//! controller intent. The analytic terrain oracle supplies the Y coordinate
//! when the trail mesh is rebuilt.
//!
//! The history is deliberately bounded and session-scoped. It is cleared at
//! [`lunco_core::SceneTeardown`], reset when the active physics frame changes,
//! and rendered as transient mesh geometry parented to that frame. No trail
//! state is authored into USD or retained across Twin replacement.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

use avian3d::prelude::Position;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::coords::GridPos;
use lunco_core::{ActivePhysicsFrame, MobilityRoot};
use lunco_render::{PbrLook, SurfaceAlpha};

use super::waypoint_click::build_ribbon_mesh;

/// Minimum horizontal travel before a new history sample is admitted.
const TRAIL_SAMPLE_SPACING_M: f64 = 0.5;
/// Maximum retained samples per vehicle. At the sampling spacing this bounds
/// the visible history to roughly 512 m, even on a rover that never stops.
const TRAIL_MAX_POINTS: usize = 1024;
/// The trail is an annotation rather than a road or terrain deformation.
const TRAIL_HALF_WIDTH_M: f32 = 0.16;
const TRAIL_SURFACE_CLEARANCE_M: f32 = 0.09;

/// Physics-frame motion history for one topology-derived vehicle.
#[derive(Component, Clone, Debug, Default)]
pub(crate) struct VehicleTrailHistory {
    frame: Option<Entity>,
    points: VecDeque<DVec3>,
}

impl VehicleTrailHistory {
    fn clear(&mut self) {
        self.frame = None;
        self.points.clear();
    }

    /// Admit one solved pose, filling a large fixed-update gap with points on
    /// the solved X/Z segment. The interpolation is only a bounded visual
    /// approximation; the endpoints still come from Avian's actual pose.
    fn record(&mut self, frame: Entity, position: DVec3) -> bool {
        if !position.is_finite() {
            return false;
        }
        if self.frame != Some(frame) {
            self.clear();
            self.frame = Some(frame);
        }

        let Some(previous) = self.points.back().copied() else {
            self.points.push_back(position);
            return true;
        };
        let distance = DVec3::new(position.x - previous.x, 0.0, position.z - previous.z).length();
        if distance < TRAIL_SAMPLE_SPACING_M {
            return false;
        }

        let steps = (distance / TRAIL_SAMPLE_SPACING_M).ceil().max(1.0) as usize;
        for step in 1..=steps {
            let point = previous.lerp(position, step as f64 / steps as f64);
            self.points.push_back(point);
            while self.points.len() > TRAIL_MAX_POINTS {
                self.points.pop_front();
            }
        }
        true
    }
}

/// One transient mesh for one vehicle's sampled path.
#[derive(Component)]
pub(crate) struct VehicleTrailMesh {
    vehicle: Entity,
    signature: u64,
}

/// The change-built render snapshot. Mesh reconciliation consumes this; it
/// does not inspect physics, terrain, or scene topology itself.
#[derive(Resource, Default)]
pub(crate) struct TrailVisualProjection {
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
    trails: HashMap<Entity, Vec<DVec3>>,
}

/// Shared ticket for sampling/projection changes, including removed scene
/// entities. It prevents a removed history from leaving a stale mesh behind.
#[derive(Resource, Default)]
pub(crate) struct TrailProjectionRebuildRequested {
    pending: bool,
}

/// Add the presentation history to every topology-derived vehicle. `MobilityRoot`
/// is the authored-vehicle capability projected by the USD simulation owner;
/// this does not classify objects by a name or make a second rover registry.
pub(crate) fn ensure_vehicle_trail_history(
    q_vehicles: Query<Entity, (With<MobilityRoot>, Without<VehicleTrailHistory>)>,
    mut commands: Commands,
) {
    for vehicle in q_vehicles.iter() {
        commands
            .entity(vehicle)
            .insert(VehicleTrailHistory::default());
    }
}

/// Sample the solved Avian pose after the fixed simulation step. The system is
/// intentionally bounded by the number of vehicle roots and only allocates when
/// a vehicle has moved at least one sample spacing.
pub(crate) fn sample_vehicle_trails(
    active_frame: Res<ActivePhysicsFrame>,
    mut q_vehicles: Query<(&Position, &mut VehicleTrailHistory), With<MobilityRoot>>,
) {
    for (position, mut history) in q_vehicles.iter_mut() {
        history.record(active_frame.0, position.0);
    }
}

/// Arm a projection rebuild from authoritative changes. In steady state this
/// is only change detection and one surface-key comparison; terrain sampling
/// is deferred to [`rebuild_vehicle_trail_projection`].
pub(crate) fn arm_trail_projection_rebuild(
    mut request: ResMut<TrailProjectionRebuildRequested>,
    q_vehicles: Query<
        (),
        (
            With<MobilityRoot>,
            Or<(Changed<Position>, Changed<VehicleTrailHistory>)>,
        ),
    >,
    mut removed_histories: RemovedComponents<VehicleTrailHistory>,
    active_frame: Res<ActivePhysicsFrame>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    projection: Res<TrailVisualProjection>,
) {
    if !q_vehicles.is_empty()
        || removed_histories.read().next().is_some()
        || active_frame.is_changed()
        || projection.frame != Some(active_frame.0)
        || projection.surface != surface.surface_key()
    {
        request.pending = true;
    }
}

pub(crate) fn trail_projection_rebuild_is_pending(
    request: Res<TrailProjectionRebuildRequested>,
) -> bool {
    request.pending
}

fn project_trail_to_surface(
    points: impl Iterator<Item = DVec3>,
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    surface_present: bool,
) -> Option<Vec<DVec3>> {
    if !surface_present {
        // A trail needs a ground owner. Do not draw an apparently grounded
        // fallback at the chassis Y when the scene has no analytic surface.
        return None;
    }
    points
        .map(|point| {
            surface
                .height_at(GridPos(point))
                .map(|height| DVec3::new(point.x, height, point.z))
        })
        .collect()
}

/// Convert all retained physics-frame history into one atomic render snapshot.
/// If any sample is outside the active DEM footprint, the trail fails closed
/// for that vehicle instead of drawing a chord through unknown terrain.
pub(crate) fn rebuild_vehicle_trail_projection(
    active_frame: Res<ActivePhysicsFrame>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    q_vehicles: Query<(Entity, &VehicleTrailHistory), With<MobilityRoot>>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
    mut projection: ResMut<TrailVisualProjection>,
) {
    request.pending = false;
    let frame = active_frame.0;
    let surface_key = surface.surface_key();
    let mut trails = HashMap::new();
    if surface_key.is_some() {
        for (vehicle, history) in q_vehicles.iter() {
            if history.frame != Some(frame) || history.points.len() < 2 {
                continue;
            }
            let Some(points) =
                project_trail_to_surface(history.points.iter().copied(), &surface, true)
            else {
                continue;
            };
            if points.len() >= 2 {
                trails.insert(vehicle, points);
            }
        }
    }
    projection.frame = Some(frame);
    projection.surface = surface_key;
    projection.trails = trails;
}

fn trail_signature(
    vehicle: Entity,
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
    points: &[DVec3],
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    vehicle.hash(&mut hash);
    frame.hash(&mut hash);
    surface.hash(&mut hash);
    for point in points {
        point.x.to_bits().hash(&mut hash);
        point.y.to_bits().hash(&mut hash);
        point.z.to_bits().hash(&mut hash);
    }
    hash.finish()
}

fn trail_look() -> PbrLook {
    PbrLook {
        base_color: LinearRgba::new(0.28, 0.14, 0.05, 0.78),
        emissive: LinearRgba::new(0.06, 0.025, 0.008, 1.0),
        alpha: SurfaceAlpha::Blend,
        unlit: true,
        double_sided: true,
        no_shadow_cast: true,
        ..default()
    }
}

/// Reconcile transient trail meshes from the snapshot. `build_ribbon_mesh`
/// owns the frame-coherent ribbon tangent and turn handling shared with route
/// annotations; this consumer only supplies trail width and appearance.
pub(crate) fn sync_vehicle_trail_meshes(
    projection: Res<TrailVisualProjection>,
    q_existing: Query<(Entity, &VehicleTrailMesh, &Mesh3d, &ChildOf)>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(frame) = projection.frame else {
        for (entity, ..) in q_existing.iter() {
            commands.entity(entity).try_despawn();
        }
        return;
    };
    let Ok(grid) = q_grids.get(frame) else {
        for (entity, ..) in q_existing.iter() {
            commands.entity(entity).try_despawn();
        }
        return;
    };

    let mut existing = HashMap::new();
    for (entity, trail, mesh, parent) in q_existing.iter() {
        let key = trail.vehicle;
        let value = (entity, trail.signature, mesh.0.clone(), parent.parent());
        if let Some((stale, ..)) = existing.insert(key, value) {
            commands.entity(stale).try_despawn();
        }
    }

    for (&vehicle, points) in &projection.trails {
        let signature = trail_signature(vehicle, projection.frame, projection.surface, points);
        let previous = existing.remove(&vehicle);
        let Some(anchor) = points.first().copied() else {
            continue;
        };
        let Some(new_mesh) = build_ribbon_mesh(
            points,
            anchor,
            TRAIL_HALF_WIDTH_M,
            TRAIL_SURFACE_CLEARANCE_M,
        ) else {
            if let Some((entity, ..)) = previous {
                commands.entity(entity).try_despawn();
            }
            continue;
        };

        if let Some((entity, old_signature, handle, parent)) = previous {
            if old_signature == signature && parent == frame {
                continue;
            }
            if parent == frame {
                if let Some(mut mesh) = meshes.get_mut(&handle) {
                    *mesh = new_mesh;
                    let (cell, local) = grid.translation_to_grid(anchor);
                    commands.entity(entity).try_insert((
                        trail_look(),
                        cell,
                        Transform::from_translation(local),
                        VehicleTrailMesh { vehicle, signature },
                    ));
                    continue;
                }
            }
            commands.entity(entity).try_despawn();
        }

        let (cell, local) = grid.translation_to_grid(anchor);
        commands.spawn((
            Mesh3d(meshes.add(new_mesh)),
            trail_look(),
            cell,
            Transform::from_translation(local),
            GlobalTransform::default(),
            ChildOf(frame),
            VehicleTrailMesh { vehicle, signature },
        ));
    }

    for (_, (entity, ..)) in existing {
        commands.entity(entity).try_despawn();
    }
}

/// Remove both the visual meshes and retained history at the scene/Twin
/// boundary. This prevents an entity-id reuse from inheriting another Twin's
/// path and leaves the next scene's projection empty until it records motion.
pub(crate) fn clear_vehicle_trails(
    mut projection: ResMut<TrailVisualProjection>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
    mut q_histories: Query<&mut VehicleTrailHistory>,
    q_meshes: Query<Entity, With<VehicleTrailMesh>>,
    mut commands: Commands,
) {
    projection.frame = None;
    projection.surface = None;
    projection.trails.clear();
    request.pending = true;
    for mut history in q_histories.iter_mut() {
        history.clear();
    }
    for entity in q_meshes.iter() {
        commands.entity(entity).try_despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_samples_long_motion_and_stays_bounded() {
        let frame = Entity::PLACEHOLDER;
        let mut history = VehicleTrailHistory::default();
        assert!(history.record(frame, DVec3::ZERO));
        assert!(history.record(frame, DVec3::new(10.0, 5.0, 0.0)));
        assert_eq!(history.points.front().copied(), Some(DVec3::ZERO));
        assert_eq!(
            history.points.back().copied(),
            Some(DVec3::new(10.0, 5.0, 0.0))
        );
        assert!(history.points.len() > 2);

        for i in 0..(TRAIL_MAX_POINTS + 128) {
            history.record(frame, DVec3::new(10.0 + i as f64, 0.0, 0.0));
        }
        assert_eq!(history.points.len(), TRAIL_MAX_POINTS);
    }

    #[test]
    fn history_resets_when_the_active_physics_frame_changes() {
        let first = Entity::from_bits(1);
        let second = Entity::from_bits(2);
        let mut history = VehicleTrailHistory::default();
        history.record(first, DVec3::new(10.0, 0.0, 20.0));
        history.record(first, DVec3::new(12.0, 0.0, 20.0));
        assert_eq!(history.points.len(), 5);

        history.record(second, DVec3::new(-3.0, 0.0, 4.0));
        assert_eq!(history.frame, Some(second));
        assert_eq!(history.points.as_slices().0, &[DVec3::new(-3.0, 0.0, 4.0)]);
    }

    #[test]
    fn trail_mesh_is_a_surface_ribbon_with_turn_safe_vertices() {
        let points = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(3.0, 1.0, 0.0),
            DVec3::new(3.0, 2.0, 3.0),
            DVec3::new(0.0, 3.0, 3.0),
        ];
        let mesh = build_ribbon_mesh(
            &points,
            points[0],
            TRAIL_HALF_WIDTH_M,
            TRAIL_SURFACE_CLEARANCE_M,
        )
        .expect("a four-point trail has ribbon geometry");
        assert_eq!(mesh.count_vertices(), points.len() * 2);
        assert_eq!(
            mesh.primitive_topology(),
            bevy::mesh::PrimitiveTopology::TriangleList
        );
    }
}
