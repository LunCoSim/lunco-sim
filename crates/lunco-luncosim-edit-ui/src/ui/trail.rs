//! Contact-derived, bounded motion history. DEM tracks use the terrain's own
//! fragments; ordinary static supports use solved contact-plane ribbons.
//! Physics records wheel contacts, never chassis intent or render transforms.

use avian3d::prelude::{ColliderOf, Collisions, Position, RayHits, RigidBody, Rotation};
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::MobilityRoot;
use lunco_mobility::wheel_kinematics::wheel_hub_pose;
use lunco_mobility::{
    JointedWheelTire, Suspension, WheelBodyMount, WheelRaycast, WheelRaycastResultsSet,
    raycast_contact_point,
};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_spatial::ActivePhysicsFrame;
use lunco_spatial::coords::{GridPos, GridRot};
use lunco_terrain_surface::annotations::SurfaceCurveAnnotation;
use lunco_terrain_surface::{ColliderTileOf, DemHeightField};
use lunco_usd_geometry::ribbon::{RibbonPoint, build_ribbon_mesh};
use lunco_usd_sim_core::PhysicalWheel;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// Fixed history spacing; a separate moving endpoint renders sub-spacing motion.
const TRAIL_SAMPLE_SPACING_M: f64 = 0.5;
/// Presentation history budget, without increasing contact sampling density.
#[derive(Resource)]
pub struct VehicleTrailSettings {
    /// Default retains about 16 km at the unchanged half-metre spacing.
    /// Valid range 2..=32768; overflow retires only the oldest recorded points.
    pub max_points_per_wheel: usize,
}
impl Default for VehicleTrailSettings {
    fn default() -> Self {
        Self {
            max_points_per_wheel: 32768,
        }
    }
}
const TRAIL_SURFACE_CLEARANCE_M: f32 = 0.02;
const TRAIL_MIN_SUPPORT_NORMAL_Y: f64 = 0.2;

#[derive(Component, Clone, Debug, Default)]
pub(crate) struct VehicleTrailHistory {
    frame: Option<Entity>,
    lanes: HashMap<Entity, WheelTrailHistory>,
}

#[derive(Clone, Debug, Default)]
struct WheelTrailHistory {
    points: VecDeque<TrailContact>,
    connected: bool,
    stroke: u64,
    width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TrailContact {
    point: DVec3,
    normal: DVec3,
    support: Entity,
    terrain: Option<Entity>,
    stroke: u64,
}

impl TrailContact {
    fn new(point: DVec3, normal: DVec3, support: Entity, terrain: Option<Entity>) -> Option<Self> {
        if !point.is_finite() {
            return None;
        }
        Some(Self {
            point,
            normal: support_normal(normal)?,
            support,
            terrain,
            stroke: 0,
        })
    }
}

impl VehicleTrailHistory {
    fn clear(&mut self) {
        self.frame = None;
        self.lanes.clear();
    }

    fn record(
        &mut self,
        frame: Entity,
        wheel: Entity,
        width: f64,
        mut contact: TrailContact,
        max_points: usize,
    ) -> bool {
        if self.frame != Some(frame) {
            self.clear();
            self.frame = Some(frame);
        }
        let lane = self.lanes.entry(wheel).or_default();
        let changed_width = lane.width != width;
        lane.width = width;
        let same_support = lane
            .points
            .back()
            .is_some_and(|previous| previous.support == contact.support);
        if !lane.connected || !same_support {
            lane.stroke += 1;
            lane.connected = true;
            contact.stroke = lane.stroke;
            lane.points.push_back(contact);
        } else {
            contact.stroke = lane.stroke;
            let previous = *lane.points.back().unwrap();
            let delta = contact.point - previous.point;
            let distance = DVec3::new(delta.x, 0.0, delta.z).length();
            if distance < 0.001 {
                return changed_width;
            }
            // Preserve one fixed anchor and move only the live endpoint until
            // another half metre is covered. There is no gap interpolation.
            let anchor = lane
                .points
                .iter()
                .rev()
                .nth(1)
                .filter(|p| p.stroke == lane.stroke);
            if anchor.is_some_and(|p| {
                let delta = contact.point - p.point;
                DVec3::new(delta.x, 0.0, delta.z).length() < TRAIL_SAMPLE_SPACING_M
            }) {
                *lane.points.back_mut().unwrap() = contact;
            } else {
                lane.points.push_back(contact);
            }
        }
        while lane.points.len() > max_points {
            lane.points.pop_front();
        }
        true
    }
}

#[derive(Component)]
pub(crate) struct VehicleTrailVisual {
    vehicle: Entity,
    wheel: Entity,
    stroke: u64,
    terrain: Option<Entity>,
    signature: u64,
}

#[derive(Resource, Default)]
pub(crate) struct TrailVisualProjection {
    frame: Option<Entity>,
    surface: Option<(Entity, u64)>,
    trails: HashMap<Entity, Vec<TrailLane>>,
}

#[derive(Clone, Debug)]
struct TrailLane {
    wheel: Entity,
    stroke: u64,
    width: f64,
    terrain: Option<Entity>,
    points: Vec<RibbonPoint>,
    segments: Vec<[DVec3; 2]>,
}

fn mobility_root(
    entity: Entity,
    roots: &Query<(), With<MobilityRoot>>,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut current = entity;
    let mut visited = HashSet::new();
    loop {
        if roots.contains(current) {
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
    bodies: &Query<&RigidBody>,
    collider_of: &Query<&ColliderOf>,
) -> Option<Entity> {
    let body = collider_of.get(collider).map_or(collider, |c| c.body);
    bodies
        .get(body)
        .is_ok_and(RigidBody::is_static)
        .then_some(body)
}

fn support_owner(
    collider: Entity,
    body: Entity,
    tiles: &Query<&ColliderTileOf>,
    terrains: &Query<(), With<DemHeightField>>,
) -> (Entity, Option<Entity>) {
    let terrain = tiles
        .get(collider)
        .or_else(|_| tiles.get(body))
        .map(|tile| tile.0)
        .ok()
        .or_else(|| terrains.contains(body).then_some(body));
    (terrain.unwrap_or(body), terrain)
}

fn valid_raycast_contact(
    wheel: &WheelRaycast,
    suspension: &Suspension,
    hits: &RayHits,
    mount: &WheelBodyMount,
    bodies: &Query<(&Position, &Rotation)>,
    rigid_bodies: &Query<&RigidBody>,
    collider_of: &Query<&ColliderOf>,
    tiles: &Query<&ColliderTileOf>,
    terrains: &Query<(), With<DemHeightField>>,
) -> Option<TrailContact> {
    if !wheel.last_normal_force.is_finite() || wheel.last_normal_force < 1.0 {
        return None;
    }
    // Select the same first nondegenerate hit as the suspension solver. A
    // rejected foreground hit never gives a farther surface contact authority.
    let hit = hits
        .iter_sorted()
        .find(|h| h.normal.is_finite() && h.normal.length_squared() > 1.0e-12)?;
    if !hit.distance.is_finite() || hit.distance >= suspension.rest_length {
        return None;
    }
    let support_body = static_body_for_collider(hit.entity, rigid_bodies, collider_of)?;
    let normal = support_normal(hit.normal)?;
    let (position, rotation) = bodies.get(mount.body).ok()?;
    let (hub, heading) = wheel_hub_pose(
        GridPos(position.0),
        GridRot(rotation.0),
        mount.local.translation.as_dvec3(),
        mount.local.rotation.as_dquat() * wheel.heading_rotation,
    );
    if (heading.0 * DVec3::Y).dot(normal) <= TRAIL_MIN_SUPPORT_NORMAL_Y {
        return None;
    }
    let point = raycast_contact_point(
        hub.0,
        heading.0,
        suspension.rest_length,
        wheel.wheel_radius,
        hit.distance,
    );
    let (support, terrain) = support_owner(hit.entity, support_body, tiles, terrains);
    TrailContact::new(point, normal, support, terrain)
}

fn physical_wheel_contact_point(
    collisions: &Collisions,
    wheel: Entity,
    bodies: &Query<&RigidBody>,
    collider_of: &Query<&ColliderOf>,
    tiles: &Query<&ColliderTileOf>,
    terrains: &Query<(), With<DemHeightField>>,
) -> Option<TrailContact> {
    let mut supports: HashMap<Entity, (DVec3, DVec3, f64, Option<Entity>)> = HashMap::new();
    for pair in collisions.collisions_with(wheel) {
        if !pair.is_touching() {
            continue;
        }
        let first = pair.body1 == Some(wheel);
        if !first && pair.body2 != Some(wheel) {
            continue;
        }
        let collider = if first {
            pair.collider2
        } else {
            pair.collider1
        };
        let Some(body) = static_body_for_collider(collider, bodies, collider_of) else {
            continue;
        };
        let (support, terrain) = support_owner(collider, body, tiles, terrains);
        for manifold in &pair.manifolds {
            let Some(normal) = lunco_mobility::contact_normal_for_body(pair, manifold, wheel)
                .and_then(support_normal)
            else {
                continue;
            };
            for point in &manifold.points {
                let impulse = point.normal_impulse;
                if impulse.is_finite() && impulse > 0.0 && point.point.is_finite() {
                    let sum =
                        supports
                            .entry(support)
                            .or_insert((DVec3::ZERO, DVec3::ZERO, 0.0, terrain));
                    sum.0 += point.point * impulse;
                    sum.1 += normal * impulse;
                    sum.2 += impulse;
                }
            }
        }
    }
    let (support, (point, normal, impulse, terrain)) =
        supports.into_iter().max_by(|(a, x), (b, y)| {
            x.2.total_cmp(&y.2)
                .then_with(|| a.to_bits().cmp(&b.to_bits()))
        })?;
    TrailContact::new(point / impulse, normal / impulse, support, terrain)
}

#[derive(Resource, Default)]
pub(crate) struct TrailProjectionRebuildRequested {
    pending: bool,
}

pub struct VehicleTrailPlugin;
impl Plugin for VehicleTrailPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<lunco_api::queries::ApiQueryRegistry>();
        app.world_mut()
            .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
            .register(InspectVehicleTrailProvider);
        app.init_resource::<VehicleTrailSettings>()
            .init_resource::<TrailVisualProjection>()
            .init_resource::<TrailProjectionRebuildRequested>()
            .add_systems(lunco_core::SceneTeardown, clear_vehicle_trails)
            .add_systems(
                FixedPostUpdate,
                sample_vehicle_trails.after(WheelRaycastResultsSet),
            )
            .add_systems(
                Update,
                (
                    ensure_vehicle_trail_history,
                    refresh_vehicle_trail_widths,
                    arm_trail_projection_rebuild,
                    rebuild_vehicle_trail_projection.run_if(trail_projection_rebuild_is_pending),
                    sync_vehicle_trail_visuals.run_if(resource_changed::<TrailVisualProjection>),
                )
                    .chain()
                    .in_set(lunco_core::RuntimeCycleSet::Visualization)
                    .before(lunco_terrain_surface::annotations::SurfaceAnnotationSet::Prepare),
            );
    }
}

pub(crate) fn ensure_vehicle_trail_history(
    q_vehicles: Query<Entity, (With<MobilityRoot>, Without<VehicleTrailHistory>)>,
    mut commands: Commands,
) {
    for vehicle in &q_vehicles {
        commands
            .entity(vehicle)
            .try_insert(VehicleTrailHistory::default());
    }
}

pub(crate) fn sample_vehicle_trails(
    active_frame: Res<ActivePhysicsFrame>,
    settings: Res<VehicleTrailSettings>,
    mut histories: Query<&mut VehicleTrailHistory, With<MobilityRoot>>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
    roots: Query<(), With<MobilityRoot>>,
    parents: Query<&ChildOf>,
    bodies: Query<(&Position, &Rotation)>,
    rigid_bodies: Query<&RigidBody>,
    collider_of: Query<&ColliderOf>,
    raycast_wheels: Query<(
        Entity,
        &WheelRaycast,
        &Suspension,
        &RayHits,
        &WheelBodyMount,
    )>,
    physical_wheels: Query<(&PhysicalWheel, Entity), With<JointedWheelTire>>,
    collisions: Collisions,
    tiles: Query<&ColliderTileOf>,
    terrains: Query<(), With<DemHeightField>>,
) {
    if !(2..=32768).contains(&settings.max_points_per_wheel) {
        if settings.is_changed() {
            warn!("vehicle trail point budget must be between 2 and 32768; recording stopped");
        }
        for mut history in &mut histories {
            if history.frame.is_some() {
                history.clear();
                request.pending = true;
            }
        }
        return;
    }
    let mut active_wheels = HashSet::new();
    let mut contacting = HashSet::new();
    let mut samples = Vec::new();
    for (entity, wheel, suspension, hits, mount) in &raycast_wheels {
        active_wheels.insert(entity);
        let Some(vehicle) = mobility_root(entity, &roots, &parents) else {
            continue;
        };
        if let Some(contact) = valid_raycast_contact(
            wheel,
            suspension,
            hits,
            mount,
            &bodies,
            &rigid_bodies,
            &collider_of,
            &tiles,
            &terrains,
        ) {
            samples.push((vehicle, entity, wheel.wheel_width, contact));
        }
    }
    for (wheel, entity) in &physical_wheels {
        active_wheels.insert(entity);
        let Some(vehicle) = mobility_root(entity, &roots, &parents) else {
            continue;
        };
        // The carrier attitude has no wheel-spin term. Reject roof/side contact
        // even when a wheel collider happens to retain an upward manifold.
        let mut parent = parents.get(entity).ok().map(ChildOf::parent);
        let mut upright = false;
        while let Some(entity) = parent {
            if let Ok((_, rotation)) = bodies.get(entity) {
                upright = (rotation.0 * DVec3::Y).y > TRAIL_MIN_SUPPORT_NORMAL_Y;
                break;
            }
            parent = parents.get(entity).ok().map(ChildOf::parent);
        }
        if !upright {
            continue;
        }
        if let Some(contact) = physical_wheel_contact_point(
            &collisions,
            entity,
            &rigid_bodies,
            &collider_of,
            &tiles,
            &terrains,
        ) {
            samples.push((vehicle, entity, f64::from(wheel.wheel_width), contact));
        }
    }
    for (vehicle, wheel, width, contact) in samples {
        if !width.is_finite() || width <= 0.0 {
            continue;
        }
        contacting.insert(wheel);
        if let Ok(mut history) = histories.get_mut(vehicle) {
            if history.bypass_change_detection().record(
                active_frame.0,
                wheel,
                width,
                contact,
                settings.max_points_per_wheel,
            ) {
                history.set_changed();
                request.pending = true;
            }
        }
    }
    for mut history in &mut histories {
        let data = history.bypass_change_detection();
        if data.frame.is_some_and(|frame| frame != active_frame.0) {
            data.clear();
            request.pending = true;
        }
        let old_count = data.lanes.len();
        data.lanes.retain(|wheel, _| active_wheels.contains(wheel));
        if data.lanes.len() != old_count {
            request.pending = true;
        }
        for (&wheel, lane) in &mut data.lanes {
            if !contacting.contains(&wheel) {
                lane.connected = false;
            }
        }
    }
}

/// Width edits and removed wheel realizations are presentation invalidations
/// even while physics is paused. Broad wheel-component changes are not.
fn refresh_vehicle_trail_widths(
    mut histories: Query<&mut VehicleTrailHistory>,
    wheels: Query<&WheelRaycast>,
    physical: Query<&PhysicalWheel>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
) {
    for mut history in &mut histories {
        let data = history.bypass_change_detection();
        let mut changed = false;
        data.lanes.retain(|wheel, lane| {
            let width = wheels
                .get(*wheel)
                .map(|wheel| wheel.wheel_width)
                .ok()
                .or_else(|| {
                    physical
                        .get(*wheel)
                        .ok()
                        .map(|wheel| f64::from(wheel.wheel_width))
                });
            let Some(width) = width.filter(|w| w.is_finite() && *w > 0.0) else {
                changed = true;
                return false;
            };
            if lane.width != width {
                lane.width = width;
                changed = true;
            }
            true
        });
        if changed {
            history.set_changed();
            request.pending = true;
        }
    }
}

pub(crate) fn arm_trail_projection_rebuild(
    mut request: ResMut<TrailProjectionRebuildRequested>,
    mut removed: RemovedComponents<VehicleTrailHistory>,
    active_frame: Res<ActivePhysicsFrame>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    projection: Res<TrailVisualProjection>,
) {
    if removed.read().count() > 0
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

pub(crate) fn rebuild_vehicle_trail_projection(
    active_frame: Res<ActivePhysicsFrame>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    histories: Query<(Entity, &VehicleTrailHistory), With<MobilityRoot>>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
    mut projection: ResMut<TrailVisualProjection>,
) {
    request.pending = false;
    let mut trails = HashMap::new();
    for (vehicle, history) in &histories {
        if history.frame != Some(active_frame.0) {
            continue;
        }
        let mut lanes = Vec::new();
        for (&wheel, lane) in &history.lanes {
            let mut run: Option<TrailLane> = None;
            for point in &lane.points {
                if run.as_ref().is_some_and(|r| r.stroke != point.stroke) {
                    let previous = run.take().unwrap();
                    if previous.points.len() >= 2 {
                        lanes.push(previous);
                    }
                }
                let run = run.get_or_insert_with(|| TrailLane {
                    wheel,
                    stroke: point.stroke,
                    width: lane.width,
                    terrain: point.terrain,
                    points: Vec::new(),
                    segments: Vec::new(),
                });
                if let Some(previous) = run.points.last() {
                    run.segments.push([previous.position, point.point]);
                }
                run.points.push(RibbonPoint {
                    position: point.point,
                    normal: point.normal,
                });
            }
            if let Some(run) = run {
                if run.points.len() >= 2 {
                    lanes.push(run);
                }
            }
        }
        // One streaming source per wheel/terrain keeps the source identity
        // stable across takeoff and landing. Explicit segments preserve breaks.
        let mut ground: HashMap<(Entity, Entity), TrailLane> = HashMap::new();
        let mut supports = Vec::new();
        for mut lane in lanes {
            if let Some(terrain) = lane.terrain {
                lane.stroke = 0;
                if let Some(existing) = ground.get_mut(&(lane.wheel, terrain)) {
                    existing.points.extend(lane.points);
                    existing.segments.extend(lane.segments);
                } else {
                    ground.insert((lane.wheel, terrain), lane);
                }
            } else {
                supports.push(lane);
            }
        }
        let mut lanes = supports;
        lanes.extend(ground.into_values());
        lanes.sort_by_key(|lane| (lane.wheel, lane.stroke));
        if !lanes.is_empty() {
            trails.insert(vehicle, lanes);
        }
    }
    projection.frame = Some(active_frame.0);
    projection.surface = surface.surface_key();
    projection.trails = trails;
}

fn trail_signature(lane: &TrailLane) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    lane.width.to_bits().hash(&mut hash);
    lane.terrain.hash(&mut hash);
    for pair in &lane.segments {
        for point in pair {
            for value in point.to_array() {
                value.to_bits().hash(&mut hash);
            }
        }
    }
    for p in &lane.points {
        for v in p.position.to_array().into_iter().chain(p.normal.to_array()) {
            v.to_bits().hash(&mut hash);
        }
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

pub(crate) fn sync_vehicle_trail_visuals(
    projection: Res<TrailVisualProjection>,
    existing_query: Query<(Entity, &VehicleTrailVisual, &ChildOf, Option<&Mesh3d>)>,
    grids: Query<&big_space::prelude::Grid>,
    surface: lunco_terrain_surface::GridSurfaceQuery,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(frame) = projection.frame.filter(|f| grids.contains(*f)) else {
        for (entity, ..) in &existing_query {
            commands.entity(entity).try_despawn();
        }
        return;
    };
    let grid = grids.get(frame).unwrap();
    let mut existing = HashMap::new();
    for (entity, trail, parent, mesh) in &existing_query {
        let key = (trail.vehicle, trail.wheel, trail.stroke, trail.terrain);
        if let Some((old, ..)) = existing.insert(
            key,
            (
                entity,
                trail.signature,
                parent.parent(),
                mesh.map(|m| m.0.clone()),
            ),
        ) {
            commands.entity(old).try_despawn();
        }
    }
    for (&vehicle, lanes) in &projection.trails {
        for lane in lanes {
            let key = (vehicle, lane.wheel, lane.stroke, lane.terrain);
            let previous = existing.remove(&key);
            let signature = trail_signature(lane);
            if previous
                .as_ref()
                .is_some_and(|(_, old, parent, _)| *old == signature && *parent == frame)
            {
                continue;
            }
            let marker = VehicleTrailVisual {
                vehicle,
                wheel: lane.wheel,
                stroke: lane.stroke,
                terrain: lane.terrain,
                signature,
            };
            if let Some(terrain) = lane.terrain {
                let segments: Option<Vec<_>> = lane
                    .segments
                    .iter()
                    .map(|pair| {
                        Some([
                            surface.terrain_local_point(terrain, GridPos(pair[0]))?,
                            surface.terrain_local_point(terrain, GridPos(pair[1]))?,
                        ])
                    })
                    .collect();
                let Some(segments) = segments else {
                    if let Some((entity, ..)) = previous {
                        commands.entity(entity).try_despawn();
                    }
                    continue;
                };
                let annotation = SurfaceCurveAnnotation {
                    terrain,
                    revision: signature,
                    segments: segments.into(),
                    streaming: true,
                    width_m: lane.width,
                    color: trail_look().base_color,
                };
                if let Some((entity, _, parent, _)) = previous {
                    if parent == frame {
                        commands.entity(entity).try_insert((marker, annotation));
                        continue;
                    }
                    commands.entity(entity).try_despawn();
                }
                commands.spawn((marker, annotation, ChildOf(frame)));
            } else {
                // Explicit rendering boundary: the ribbon builder consumes f32
                // half-widths, while physics/history retain authored metres.
                let half_width = (lane.width * 0.5) as f32;
                if !half_width.is_finite() || half_width <= 0.0 {
                    continue;
                }
                let anchor = lane.points[0].position;
                let Some(mesh) = build_ribbon_mesh(
                    &lane.points,
                    anchor,
                    &[half_width],
                    TRAIL_SURFACE_CLEARANCE_M,
                    false,
                ) else {
                    continue;
                };
                let (cell, local) = grid.translation_to_grid(anchor);
                if let Some((entity, _, parent, Some(handle))) = previous.as_ref() {
                    if *parent == frame {
                        if let Some(mut old) = meshes.get_mut(handle) {
                            *old = mesh;
                            commands.entity(*entity).try_insert((
                                marker,
                                trail_look(),
                                cell,
                                Transform::from_translation(local),
                            ));
                            continue;
                        }
                    }
                }
                if let Some((entity, ..)) = previous {
                    commands.entity(entity).try_despawn();
                }
                commands.spawn((
                    marker,
                    Mesh3d(meshes.add(mesh)),
                    trail_look(),
                    cell,
                    Transform::from_translation(local),
                    GlobalTransform::default(),
                    ChildOf(frame),
                ));
            }
        }
    }
    for (_, (entity, ..)) in existing {
        commands.entity(entity).try_despawn();
    }
}

pub(crate) fn clear_vehicle_trails(
    mut projection: ResMut<TrailVisualProjection>,
    mut request: ResMut<TrailProjectionRebuildRequested>,
    mut histories: Query<&mut VehicleTrailHistory>,
    trails: Query<Entity, With<VehicleTrailVisual>>,
    mut commands: Commands,
) {
    *projection = TrailVisualProjection::default();
    request.pending = true;
    for mut history in &mut histories {
        history.clear();
    }
    for entity in &trails {
        commands.entity(entity).try_despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn contact(x: f64) -> TrailContact {
        TrailContact::new(DVec3::new(x, 0.0, 0.0), DVec3::Y, Entity::PLACEHOLDER, None).unwrap()
    }
    #[test]
    fn moving_endpoint_preserves_sparse_history_and_contact_breaks() {
        let mut history = VehicleTrailHistory::default();
        let frame = Entity::PLACEHOLDER;
        let wheel = Entity::from_bits(7);
        for x in [0.0, 0.1, 0.2, 0.3] {
            assert!(history.record(frame, wheel, 0.28, contact(x), 32768));
        }
        assert_eq!(history.lanes[&wheel].points.len(), 2);
        assert_eq!(history.lanes[&wheel].points.back().unwrap().point.x, 0.3);
        history.record(frame, wheel, 0.28, contact(0.6), 32768);
        assert_eq!(history.lanes[&wheel].points.len(), 3);
        history.lanes.get_mut(&wheel).unwrap().connected = false;
        history.record(frame, wheel, 0.28, contact(20.0), 32768);
        history.record(frame, wheel, 0.28, contact(20.1), 32768);
        let lane = &history.lanes[&wheel];
        assert_ne!(lane.points[2].stroke, lane.points[3].stroke);
        assert_eq!(lane.points[2].point.x, 0.6);
        assert_eq!(lane.points[3].point.x, 20.0);
        assert_eq!(lane.points[4].point.x, 20.1);
    }
    #[test]
    fn production_history_retains_multiple_kilometres_without_denser_sampling() {
        let mut history = VehicleTrailHistory::default();
        let wheel = Entity::from_bits(7);
        let limit = VehicleTrailSettings::default().max_points_per_wheel;
        for i in 0..=10000 {
            history.record(
                Entity::PLACEHOLDER,
                wheel,
                0.3,
                contact(i as f64 * 0.5),
                limit,
            );
        }
        let lane = &history.lanes[&wheel];
        assert_eq!(lane.points.front().unwrap().point.x, 0.0);
        assert_eq!(lane.points.back().unwrap().point.x, 5000.0);
        assert_eq!(lane.points.len(), 10001);
    }
    #[test]
    fn history_is_bounded_and_resets_at_frame_boundary() {
        let mut history = VehicleTrailHistory::default();
        let frame = Entity::PLACEHOLDER;
        let wheel = Entity::from_bits(7);
        for i in 0..110 {
            history.record(frame, wheel, 0.4, contact(i as f64), 100);
        }
        assert_eq!(history.lanes[&wheel].points.len(), 100);
        history.record(Entity::from_bits(8), wheel, 0.4, contact(0.0), 100);
        assert_eq!(history.lanes[&wheel].points.len(), 1);
    }
}

/// Read-only evidence from the same history and publication used by rendering.
/// This is presentation state, not a second wheel-contact or telemetry model.
struct InspectVehicleTrailProvider;
impl lunco_api::queries::ApiQueryProvider for InspectVehicleTrailProvider {
    fn name(&self) -> &'static str {
        "InspectVehicleTrail"
    }
    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        lunco_api_core::ApiQuerySchema {
            name: "InspectVehicleTrail".into(),
            description: Some("Inspect bounded solved-contact trail history, wheel widths and terrain publication.".into()),
            parameters: Some(vec![lunco_api_core::ApiQueryParameterSchema {
                name: "id".into(), type_name: "u64".into(), required: true,
                description: "Stable identity of the topology-derived vehicle.".into(), allowed_values: None,
            }]), exactly_one_of: Vec::new(),
            response: Some("{ lanes: [{ wheel_id, width_m, wheel_width_m, render_width_m, contacting, sample_count, strokes: [{ id, count, start, end }], projection, published_revision, error }] }".into()),
        }
    }
    fn simulation_read_scope(
        &self,
        _: &lunco_api_core::ApiValue,
    ) -> lunco_api::queries::SimulationQueryReadScope {
        lunco_api::queries::SimulationQueryReadScope::EntityTargets
    }
    fn simulation_entity_reads(
        &self,
        params: &lunco_api_core::ApiValue,
    ) -> Vec<lunco_core::GlobalEntityId> {
        lunco_api::queries::api_param_u64(params, "id")
            .map(lunco_core::GlobalEntityId::from_raw)
            .into_iter()
            .collect()
    }
    fn execute(
        &self,
        world: &World,
        params: &lunco_api_core::ApiValue,
    ) -> lunco_api::ApiQueryResult {
        use lunco_api::ApiQueryError;
        use lunco_api_core::{ApiErrorCode, api_value};
        let id = lunco_api::queries::api_param_u64(params, "id").ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "InspectVehicleTrail requires id",
            )
        })?;
        let registry = world
            .get_resource::<lunco_api::registry::ApiEntityRegistry>()
            .ok_or_else(|| {
                ApiQueryError::new(ApiErrorCode::InternalError, "Entity registry unavailable")
            })?;
        let vehicle = registry
            .resolve(&lunco_core::GlobalEntityId::from_raw(id))
            .ok_or_else(|| {
                ApiQueryError::new(ApiErrorCode::EntityNotFound, "Vehicle identity is not live")
            })?;
        let history = world.get::<VehicleTrailHistory>(vehicle).ok_or_else(|| {
            ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                "Vehicle has no motion trail history",
            )
        })?;
        let mut lanes = Vec::new();
        let mut ordered: Vec<_> = history.lanes.iter().collect();
        ordered.sort_by_key(|(wheel, _)| **wheel);
        for (&wheel, lane) in ordered {
            let mut strokes = Vec::new();
            let mut start = 0;
            let points: Vec<_> = lane.points.iter().collect();
            while start < points.len() {
                let stroke = points[start].stroke;
                let end = (start + 1..points.len())
                    .find(|&i| points[i].stroke != stroke)
                    .unwrap_or(points.len());
                strokes.push(api_value!({"id": stroke, "count": end - start, "start": points[start].point.to_array(), "end": points[end - 1].point.to_array()}));
                start = end;
            }
            let publication = lane.points.back().and_then(|p| p.terrain).and_then(|terrain| world.get_resource::<lunco_terrain_surface::annotations::SurfaceAnnotationImages>().and_then(|images| images.published.get(&terrain)));
            let published_revision = publication.and_then(|published| {
                world.iter_entities().find_map(|entity| {
                    let marker = entity.get::<VehicleTrailVisual>()?;
                    if marker.vehicle != vehicle
                        || marker.wheel != wheel
                        || marker.terrain.is_none()
                    {
                        return None;
                    }
                    published
                        .sources
                        .iter()
                        .find(|(source, _)| *source == entity.id())
                        .map(|(_, revision)| *revision)
                })
            });
            let wheel_width = world
                .get::<WheelRaycast>(wheel)
                .map(|wheel| wheel.wheel_width)
                .or_else(|| {
                    world
                        .get::<PhysicalWheel>(wheel)
                        .map(|wheel| f64::from(wheel.wheel_width))
                });
            let render_width = world.iter_entities().find_map(|entity| {
                let marker = entity.get::<VehicleTrailVisual>()?;
                if marker.vehicle != vehicle || marker.wheel != wheel {
                    return None;
                }
                entity
                    .get::<SurfaceCurveAnnotation>()
                    .map(|source| source.width_m)
                    .or_else(|| {
                        world
                            .get_resource::<TrailVisualProjection>()
                            .and_then(|p| p.trails.get(&vehicle))
                            .and_then(|lanes| {
                                lanes.iter().find(|lane| {
                                    lane.wheel == wheel && lane.stroke == marker.stroke
                                })
                            })
                            .map(|lane| lane.width)
                    })
            });
            lanes.push(api_value!({"wheel_id": registry.api_id_for(wheel).map(|id| id.get()), "width_m": lane.width, "wheel_width_m": wheel_width, "render_width_m": render_width, "contacting": lane.connected, "sample_count": lane.points.len(), "strokes": strokes, "projection": if lane.points.back().is_some_and(|p| p.terrain.is_some()) { "terrain" } else { "contact_mesh" }, "published_revision": published_revision, "error": publication.and_then(|p| p.error.clone())}));
        }
        Ok(Some(api_value!({"lanes": lanes})))
    }
}
