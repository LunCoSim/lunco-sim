//! Bounded, terrain-conforming motion trails for topology-derived vehicles.
//!
//! A trail is presentation history, not a second vehicle state. Each lane is
//! sampled from the wheel realization's solved ground contact: raycast wheels
//! use their retained Avian ray hit and jointed wheels use Avian contact
//! manifolds. The history is stored in the active physics/grid frame and is
//! never inferred from render transforms, root motion, or controller intent.
//! The analytic terrain oracle supplies the frame-correct point and support
//! normal when the trail mesh is rebuilt.
//!
//! The history is deliberately bounded and session-scoped. It is cleared at
//! [`lunco_core::SceneTeardown`], reset when the active physics frame changes,
//! and rendered as transient mesh geometry parented to that frame. No trail
//! state is authored into USD or retained across Twin replacement.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

use avian3d::prelude::{ColliderOf, Collisions, Position, RayHits, RigidBody, Rotation};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::coords::{GridPos, GridRot};
use lunco_core::{ActivePhysicsFrame, MobilityRoot};
use lunco_mobility::wheel_kinematics::wheel_hub_pose;
use lunco_mobility::{
    raycast_contact_point, JointedWheelTire, Suspension, WheelBodyMount, WheelRaycast,
};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_sim::PhysicalWheel;

use super::waypoint_click::{build_ribbon_mesh, RibbonPoint};

/// Minimum horizontal travel before a new history sample is admitted.
const TRAIL_SAMPLE_SPACING_M: f64 = 0.5;
/// Maximum retained samples per vehicle. At the sampling spacing this bounds
/// the visible history to roughly 512 m, even on a rover that never stops.
const TRAIL_MAX_POINTS: usize = 1024;
/// The trail is an annotation rather than a road or terrain deformation.
const TRAIL_HALF_WIDTH_M: f32 = 0.16;
const TRAIL_SURFACE_CLEARANCE_M: f32 = 0.09;
/// A wheel contact must support some upward load. This excludes vertical wall
/// contacts while preserving steep but physically driveable static surfaces.
const TRAIL_MIN_SUPPORT_NORMAL_Y: f64 = 0.2;
const TRAIL_SURFACE_NORMAL_EPS_M: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrailSurfaceMode {
    /// The Avian contact point is already the authoritative surface sample.
    PhysicsContact,
    /// Reproject the contact's horizontal coordinates through the DEM oracle.
    AnalyticTerrain,
}

/// Physics-frame contact history for one topology-derived vehicle.
#[derive(Component, Clone, Debug, Default)]
pub(crate) struct VehicleTrailHistory {
    frame: Option<Entity>,
    lanes: HashMap<Entity, WheelTrailHistory>,
}

#[derive(Clone, Debug, Default)]
struct WheelTrailHistory {
    points: VecDeque<TrailContact>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TrailContact {
    point: DVec3,
    normal: DVec3,
}

impl TrailContact {
    fn new(point: DVec3, normal: DVec3) -> Option<Self> {
        if !point.is_finite() || !normal.is_finite() {
            return None;
        }
        let normal = normal.normalize_or_zero();
        (normal.length_squared() > 1.0e-12 && normal.y >= TRAIL_MIN_SUPPORT_NORMAL_Y)
            .then_some(Self { point, normal })
    }
}

impl VehicleTrailHistory {
    fn clear(&mut self) {
        self.frame = None;
        self.lanes.clear();
    }

    /// Admit one solved pose, filling a large fixed-update gap with points on
    /// the solved contact segment. The interpolation is only a bounded visual
    /// approximation; the endpoints still come from Avian's actual contact.
    fn record(&mut self, frame: Entity, wheel: Entity, contact: TrailContact) -> bool {
        if self.frame != Some(frame) {
            self.clear();
            self.frame = Some(frame);
        }

        let points = &mut self.lanes.entry(wheel).or_default().points;
        let Some(previous) = points.back().copied() else {
            points.push_back(contact);
            return true;
        };
        let distance = DVec3::new(
            contact.point.x - previous.point.x,
            0.0,
            contact.point.z - previous.point.z,
        )
        .length();
        if distance < TRAIL_SAMPLE_SPACING_M {
            return false;
        }

        let steps = (distance / TRAIL_SAMPLE_SPACING_M).ceil().max(1.0) as usize;
        for step in 1..=steps {
            let t = step as f64 / steps as f64;
            let point = previous.point.lerp(contact.point, t);
            let normal = previous.normal.lerp(contact.normal, t).normalize_or_zero();
            points.push_back(TrailContact { point, normal });
            while points.len() > TRAIL_MAX_POINTS {
                points.pop_front();
            }
        }
        true
    }
}

/// One transient mesh for one vehicle's sampled path.
#[derive(Component)]
pub(crate) struct VehicleTrailMesh {
    vehicle: Entity,
    wheel: Entity,
    signature: u64,
}

/// The change-built render snapshot. Mesh reconciliation consumes this; it
/// does not inspect physics, terrain, or scene topology itself.
#[derive(Resource, Default)]
pub(crate) struct TrailVisualProjection {
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
    trails: HashMap<Entity, Vec<TrailLane>>,
}

#[derive(Clone, Debug)]
struct TrailLane {
    wheel: Entity,
    points: Vec<RibbonPoint>,
}

/// Find the topology-derived vehicle owner for a wheel. The walk deliberately
/// uses the existing ECS hierarchy and capability marker; it does not create a
/// vehicle registry or classify names.
fn mobility_root(
    entity: Entity,
    roots: &Query<(), With<MobilityRoot>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut current = entity;
    let mut visited = HashSet::new();
    loop {
        if roots.get(current).is_ok() {
            return Some(current);
        }
        if !visited.insert(current) {
            return None;
        }
        current = parents.get(current).ok()?.parent();
    }
}

fn support_normal(normal: DVec3) -> Option<DVec3> {
    let normal = normal.normalize_or_zero();
    (normal.is_finite()
        && normal.length_squared() > 1.0e-12
        && normal.y >= TRAIL_MIN_SUPPORT_NORMAL_Y)
        .then_some(normal)
}

fn static_body_for_collider(
    collider: Entity,
    q_bodies: &Query<&RigidBody>,
    q_collider_of: &Query<&ColliderOf>,
) -> Option<Entity> {
    let body = q_collider_of
        .get(collider)
        .map_or(collider, |collider_of| collider_of.body);
    q_bodies
        .get(body)
        .ok()
        .is_some_and(|body| body.is_static())
        .then_some(body)
}

fn valid_raycast_contact(
    wheel: &WheelRaycast,
    suspension: &Suspension,
    hits: &RayHits,
    wheel_transform: &Transform,
    mount: &WheelBodyMount,
    q_bodies: &Query<(&Position, &Rotation)>,
    q_rigid_bodies: &Query<&RigidBody>,
    q_collider_of: &Query<&ColliderOf>,
) -> Option<TrailContact> {
    if wheel.last_normal_force < 1.0 {
        return None;
    }
    let hit = hits.iter_sorted().find(|hit| {
        static_body_for_collider(hit.entity, q_rigid_bodies, q_collider_of).is_some()
            && support_normal(hit.normal).is_some()
    })?;
    let normal = support_normal(hit.normal)?;
    let (body_position, body_rotation) = q_bodies.get(mount.body).ok()?;
    let (hub, rotation) = wheel_hub_pose(
        GridPos(body_position.0),
        GridRot(body_rotation.0),
        mount.local.translation.as_dvec3(),
        (mount.local.rotation * wheel_transform.rotation).as_dquat(),
    );
    let point = raycast_contact_point(
        hub.0,
        rotation.0,
        suspension.rest_length,
        wheel.wheel_radius,
        hit.distance,
    );
    TrailContact::new(point, normal)
}

/// Avian contact-point centroid for a jointed wheel. Normal impulse is used as
/// the weight so a manifold with several support points records the same
/// effective patch that the shared tire solver consumes.
fn physical_wheel_contact_point(
    collisions: &Collisions,
    wheel: Entity,
    q_rigid_bodies: &Query<&RigidBody>,
    q_collider_of: &Query<&ColliderOf>,
) -> Option<TrailContact> {
    let mut weighted = DVec3::ZERO;
    let mut weighted_normal = DVec3::ZERO;
    let mut total_impulse = 0.0;
    for pair in collisions.collisions_with(wheel) {
        if !pair.is_touching() {
            continue;
        }
        let wheel_is_body1 = pair.body1 == Some(wheel);
        let wheel_is_body2 = pair.body2 == Some(wheel);
        if !wheel_is_body1 && !wheel_is_body2 {
            continue;
        }
        let support_collider = if wheel_is_body1 {
            pair.collider2
        } else {
            pair.collider1
        };
        let support_body = if wheel_is_body1 {
            pair.body2
        } else {
            pair.body1
        }
        .or_else(|| {
            q_collider_of
                .get(support_collider)
                .ok()
                .map(|collider_of| collider_of.body)
        });
        let Some(support_body) = support_body else {
            continue;
        };
        if !q_rigid_bodies
            .get(support_body)
            .is_ok_and(RigidBody::is_static)
        {
            continue;
        }
        for manifold in &pair.manifolds {
            let Some(normal) = lunco_mobility::contact_normal_for_body(pair, manifold, wheel)
            else {
                continue;
            };
            let Some(normal) = support_normal(normal) else {
                continue;
            };
            for point in &manifold.points {
                let impulse = point.normal_impulse as f64;
                if impulse.is_finite() && impulse > 0.0 && point.point.is_finite() {
                    weighted += point.point * impulse;
                    weighted_normal += normal * impulse;
                    total_impulse += impulse;
                }
            }
        }
    }
    (total_impulse > 0.0)
        .then(|| TrailContact::new(weighted / total_impulse, weighted_normal / total_impulse))?
}

/// Shared ticket for sampling/projection changes, including removed scene
/// entities. It prevents a removed history from leaving a stale mesh behind.
#[derive(Resource, Default)]
pub(crate) struct TrailProjectionRebuildRequested {
    pending: bool,
}

/// Presentation-only trail systems shared by the interactive viewport and the
/// GPU offscreen recorder. It has no egui or picking dependency, so visual
/// acceptance renders the same trail product that the desktop viewport uses.
pub struct VehicleTrailPlugin;

impl Plugin for VehicleTrailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrailVisualProjection>()
            .init_resource::<TrailProjectionRebuildRequested>()
            .add_systems(lunco_core::SceneTeardown, clear_vehicle_trails)
            .add_systems(
                Update,
                (
                    ensure_vehicle_trail_history.before(sample_vehicle_trails),
                    sample_vehicle_trails.before(arm_trail_projection_rebuild),
                    arm_trail_projection_rebuild,
                    rebuild_vehicle_trail_projection
                        .after(arm_trail_projection_rebuild)
                        .run_if(trail_projection_rebuild_is_pending),
                    sync_vehicle_trail_meshes
                        .after(rebuild_vehicle_trail_projection)
                        .run_if(resource_changed::<TrailVisualProjection>),
                ),
            );
    }
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

/// Sample solved wheel-ground contacts after the physics step. Raycast and
/// jointed wheels share one presentation history; neither path samples a
/// render transform or the vehicle root pose.
pub(crate) fn sample_vehicle_trails(
    active_frame: Res<ActivePhysicsFrame>,
    mut q_histories: Query<&mut VehicleTrailHistory, With<MobilityRoot>>,
    q_roots: Query<(), With<MobilityRoot>>,
    q_parents: Query<&ChildOf>,
    q_bodies: Query<(&Position, &Rotation)>,
    q_rigid_bodies: Query<&RigidBody>,
    q_collider_of: Query<&ColliderOf>,
    q_raycast_wheels: Query<(
        Entity,
        &WheelRaycast,
        &Suspension,
        &RayHits,
        &Transform,
        &WheelBodyMount,
    )>,
    q_physical_wheels: Query<Entity, (With<PhysicalWheel>, With<JointedWheelTire>)>,
    collisions: Collisions,
) {
    let mut samples = Vec::new();
    let mut active_wheels = HashSet::new();
    for (wheel_entity, wheel, suspension, hits, wheel_transform, mount) in q_raycast_wheels.iter() {
        active_wheels.insert(wheel_entity);
        let Some(vehicle) = mobility_root(wheel_entity, &q_roots, &q_parents) else {
            continue;
        };
        let Some(contact) = valid_raycast_contact(
            wheel,
            suspension,
            hits,
            wheel_transform,
            mount,
            &q_bodies,
            &q_rigid_bodies,
            &q_collider_of,
        ) else {
            continue;
        };
        samples.push((vehicle, wheel_entity, contact));
    }
    for wheel_entity in q_physical_wheels.iter() {
        active_wheels.insert(wheel_entity);
        let Some(vehicle) = mobility_root(wheel_entity, &q_roots, &q_parents) else {
            continue;
        };
        let Some(contact) = physical_wheel_contact_point(
            &collisions,
            wheel_entity,
            &q_rigid_bodies,
            &q_collider_of,
        ) else {
            continue;
        };
        samples.push((vehicle, wheel_entity, contact));
    }

    for (vehicle, wheel, contact) in samples {
        if let Ok(mut history) = q_histories.get_mut(vehicle) {
            history.record(active_frame.0, wheel, contact);
        }
    }
    for mut history in q_histories.iter_mut() {
        history
            .lanes
            .retain(|wheel, _| active_wheels.contains(wheel));
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
    contacts: impl Iterator<Item = TrailContact>,
    surface: &lunco_terrain_surface::GridSurfaceQuery,
    mode: TrailSurfaceMode,
) -> Option<Vec<RibbonPoint>> {
    if mode == TrailSurfaceMode::PhysicsContact {
        // The solved physics contact is already on the authored static support
        // collider. Keep both its point and support frame.
        Some(
            contacts
                .map(|contact| RibbonPoint {
                    position: contact.point,
                    normal: contact.normal,
                })
                .collect(),
        )
    } else {
        contacts
            .map(|contact| {
                surface
                    .sample_surface(GridPos(contact.point), TRAIL_SURFACE_NORMAL_EPS_M)
                    .map(|sample| RibbonPoint {
                        position: sample.point.0,
                        normal: sample.normal,
                    })
            })
            .collect()
    }
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
    let surface_mode = if surface_key.is_some() {
        TrailSurfaceMode::AnalyticTerrain
    } else {
        TrailSurfaceMode::PhysicsContact
    };
    let mut trails: HashMap<Entity, Vec<TrailLane>> = HashMap::new();
    for (vehicle, history) in q_vehicles.iter() {
        if history.frame != Some(frame) {
            continue;
        }
        let mut lanes = history.lanes.iter().collect::<Vec<_>>();
        lanes.sort_by_key(|(wheel, _)| wheel.to_bits());
        for (&wheel, lane) in lanes {
            if lane.points.len() < 2 {
                continue;
            }
            let Some(points) =
                project_trail_to_surface(lane.points.iter().copied(), &surface, surface_mode)
            else {
                continue;
            };
            if points.len() >= 2 {
                trails
                    .entry(vehicle)
                    .or_default()
                    .push(TrailLane { wheel, points });
            }
        }
    }
    projection.frame = Some(frame);
    projection.surface = surface_key;
    projection.trails = trails;
}

fn trail_signature(
    vehicle: Entity,
    wheel: Entity,
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
    points: &[RibbonPoint],
) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    vehicle.hash(&mut hash);
    wheel.hash(&mut hash);
    frame.hash(&mut hash);
    surface.hash(&mut hash);
    for point in points {
        point.position.x.to_bits().hash(&mut hash);
        point.position.y.to_bits().hash(&mut hash);
        point.position.z.to_bits().hash(&mut hash);
        point.normal.x.to_bits().hash(&mut hash);
        point.normal.y.to_bits().hash(&mut hash);
        point.normal.z.to_bits().hash(&mut hash);
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
        let key = (trail.vehicle, trail.wheel);
        let value = (entity, trail.signature, mesh.0.clone(), parent.parent());
        if let Some((stale, ..)) = existing.insert(key, value) {
            commands.entity(stale).try_despawn();
        }
    }

    for (&vehicle, lanes) in &projection.trails {
        for lane in lanes {
            let key = (vehicle, lane.wheel);
            let signature = trail_signature(
                vehicle,
                lane.wheel,
                projection.frame,
                projection.surface,
                &lane.points,
            );
            let previous = existing.remove(&key);
            let Some(anchor) = lane.points.first().map(|point| point.position) else {
                continue;
            };
            let Some(new_mesh) = build_ribbon_mesh(
                &lane.points,
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
                            VehicleTrailMesh {
                                vehicle,
                                wheel: lane.wheel,
                                signature,
                            },
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
                VehicleTrailMesh {
                    vehicle,
                    wheel: lane.wheel,
                    signature,
                },
            ));
        }
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

    fn flat_contact(point: DVec3) -> TrailContact {
        TrailContact::new(point, DVec3::Y).expect("flat support contact")
    }

    #[test]
    fn only_upward_static_support_normals_can_leave_a_trail() {
        assert!(support_normal(DVec3::Y).is_some());
        assert!(support_normal(DVec3::new(-0.6, 0.8, 0.0)).is_some());
        assert!(support_normal(DVec3::X).is_none());
        assert!(support_normal(-DVec3::Y).is_none());
        assert!(support_normal(DVec3::NAN).is_none());
    }

    #[test]
    fn history_samples_contact_lanes_and_stays_bounded() {
        let frame = Entity::PLACEHOLDER;
        let wheel = Entity::from_bits(7);
        let mut history = VehicleTrailHistory::default();
        assert!(history.record(frame, wheel, flat_contact(DVec3::ZERO)));
        assert!(history.record(frame, wheel, flat_contact(DVec3::new(10.0, 5.0, 0.0)),));
        let points = &history.lanes[&wheel].points;
        assert_eq!(
            points.front().map(|contact| contact.point),
            Some(DVec3::ZERO)
        );
        assert_eq!(
            points.back().map(|contact| contact.point),
            Some(DVec3::new(10.0, 5.0, 0.0))
        );
        assert!(points.len() > 2);

        for i in 0..(TRAIL_MAX_POINTS + 128) {
            history.record(
                frame,
                wheel,
                flat_contact(DVec3::new(10.0 + i as f64, 0.0, 0.0)),
            );
        }
        assert_eq!(history.lanes[&wheel].points.len(), TRAIL_MAX_POINTS);
    }

    #[test]
    fn history_resets_when_the_active_physics_frame_changes() {
        let first = Entity::from_bits(1);
        let second = Entity::from_bits(2);
        let wheel = Entity::from_bits(3);
        let mut history = VehicleTrailHistory::default();
        history.record(first, wheel, flat_contact(DVec3::new(10.0, 0.0, 20.0)));
        history.record(first, wheel, flat_contact(DVec3::new(12.0, 0.0, 20.0)));
        assert_eq!(history.lanes[&wheel].points.len(), 5);

        history.record(second, wheel, flat_contact(DVec3::new(-3.0, 0.0, 4.0)));
        assert_eq!(history.frame, Some(second));
        assert_eq!(history.lanes.len(), 1);
        assert_eq!(
            history.lanes[&wheel].points.as_slices().0,
            &[flat_contact(DVec3::new(-3.0, 0.0, 4.0))]
        );
    }

    #[test]
    fn history_keeps_each_wheel_contact_on_its_own_lane() {
        let frame = Entity::PLACEHOLDER;
        let left = Entity::from_bits(11);
        let right = Entity::from_bits(12);
        let mut history = VehicleTrailHistory::default();

        history.record(frame, left, flat_contact(DVec3::new(-1.0, 0.0, 0.0)));
        history.record(frame, left, flat_contact(DVec3::new(-1.0, 0.0, -1.0)));
        history.record(frame, right, flat_contact(DVec3::new(1.0, 0.0, 0.0)));
        history.record(frame, right, flat_contact(DVec3::new(1.0, 0.0, -1.0)));

        assert_eq!(history.lanes.len(), 2);
        assert_eq!(
            history.lanes[&left]
                .points
                .front()
                .map(|contact| contact.point),
            Some(DVec3::new(-1.0, 0.0, 0.0))
        );
        assert_eq!(
            history.lanes[&right]
                .points
                .front()
                .map(|contact| contact.point),
            Some(DVec3::new(1.0, 0.0, 0.0))
        );
    }

    #[test]
    fn history_retains_and_interpolates_the_solved_support_frame() {
        let frame = Entity::PLACEHOLDER;
        let wheel = Entity::from_bits(13);
        let normal = DVec3::new(-0.6, 0.8, 0.0);
        let mut history = VehicleTrailHistory::default();

        history.record(frame, wheel, flat_contact(DVec3::ZERO));
        history.record(
            frame,
            wheel,
            TrailContact::new(DVec3::new(2.0, 0.0, 0.0), normal).unwrap(),
        );
        let samples = &history.lanes[&wheel].points;

        assert_eq!(samples.len(), 5);
        assert_eq!(samples.front().unwrap().normal, DVec3::Y);
        assert!((samples.back().unwrap().normal - normal).length() < 1.0e-12);
        assert!((samples[2].normal - DVec3::new(-0.3, 0.9, 0.0).normalize()).length() < 1.0e-12);
    }

    #[test]
    fn trail_mesh_is_a_surface_ribbon_with_turn_safe_vertices() {
        let points = [
            flat_contact(DVec3::new(0.0, 0.0, 0.0)),
            flat_contact(DVec3::new(3.0, 1.0, 0.0)),
            flat_contact(DVec3::new(3.0, 2.0, 3.0)),
            flat_contact(DVec3::new(0.0, 3.0, 3.0)),
        ];
        let points = points.map(|contact| RibbonPoint {
            position: contact.point,
            normal: contact.normal,
        });
        let mesh = build_ribbon_mesh(
            &points,
            points[0].position,
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

    #[test]
    fn trail_mesh_clearance_and_normals_follow_a_ramp_support_frame() {
        let normal = DVec3::new(-0.6, 0.8, 0.0);
        let points = [
            RibbonPoint {
                position: DVec3::ZERO,
                normal,
            },
            RibbonPoint {
                position: DVec3::new(2.0, 1.5, 0.0),
                normal,
            },
        ];
        let mesh = build_ribbon_mesh(&points, DVec3::ZERO, 0.2, 0.1).expect("ramp ribbon");
        let bevy::mesh::VertexAttributeValues::Float32x3(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .expect("ribbon positions")
        else {
            panic!("ribbon positions must be Float32x3");
        };
        let bevy::mesh::VertexAttributeValues::Float32x3(normals) = mesh
            .attribute(Mesh::ATTRIBUTE_NORMAL)
            .expect("ribbon normals")
        else {
            panic!("ribbon normals must be Float32x3");
        };

        assert!((positions[0][1] - 0.08).abs() < 1.0e-6);
        assert!((positions[0][0] - (-0.06)).abs() < 1.0e-6);
        assert!((normals[0][0] - (-0.6)).abs() < 1.0e-6);
        assert!((normals[0][1] - 0.8).abs() < 1.0e-6);
    }

    #[test]
    fn contact_points_are_retained_without_an_analytic_dem() {
        let contacts = [
            flat_contact(DVec3::new(1.0, 0.25, 2.0)),
            flat_contact(DVec3::new(2.0, 0.5, 2.0)),
        ];
        let mut app = App::new();
        let system = app.world_mut().register_system(
            move |surface: lunco_terrain_surface::GridSurfaceQuery| {
                project_trail_to_surface(
                    contacts.into_iter(),
                    &surface,
                    TrailSurfaceMode::PhysicsContact,
                )
            },
        );
        let projected = app.world_mut().run_system(system).unwrap();
        assert_eq!(
            projected,
            Some(
                contacts
                    .into_iter()
                    .map(|contact| RibbonPoint {
                        position: contact.point,
                        normal: contact.normal,
                    })
                    .collect()
            )
        );
    }

    #[test]
    fn scene_teardown_clears_history_and_projection() {
        let frame = Entity::from_bits(21);
        let wheel = Entity::from_bits(22);
        let mut history = VehicleTrailHistory::default();
        history.record(frame, wheel, flat_contact(DVec3::ZERO));
        history.record(frame, wheel, flat_contact(DVec3::X));

        let mut app = App::new();
        let vehicle = app.world_mut().spawn((MobilityRoot, history)).id();
        app.insert_resource(TrailVisualProjection {
            frame: Some(frame),
            surface: Some((frame, 1)),
            trails: HashMap::from([(
                vehicle,
                vec![TrailLane {
                    wheel,
                    points: vec![
                        RibbonPoint {
                            position: DVec3::ZERO,
                            normal: DVec3::Y,
                        },
                        RibbonPoint {
                            position: DVec3::X,
                            normal: DVec3::Y,
                        },
                    ],
                }],
            )]),
        });
        app.insert_resource(TrailProjectionRebuildRequested::default());
        app.add_systems(lunco_core::SceneTeardown, clear_vehicle_trails);

        app.world_mut().run_schedule(lunco_core::SceneTeardown);

        let projection = app.world().resource::<TrailVisualProjection>();
        assert_eq!(projection.frame, None);
        assert_eq!(projection.surface, None);
        assert!(projection.trails.is_empty());
        let history = app.world().get::<VehicleTrailHistory>(vehicle).unwrap();
        assert_eq!(history.frame, None);
        assert!(history.lanes.is_empty());
        assert!(
            app.world()
                .resource::<TrailProjectionRebuildRequested>()
                .pending
        );
    }
}
