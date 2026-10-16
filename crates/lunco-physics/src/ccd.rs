//! Continuous contact constraints share the native substep joint solve.

use crate::joint::rotation_displacement;
use lunco_core::AppSceneExt;

use crate::{
    PhysicsHolds,
    joint_solver::{
        JointSolverOrder, PhysicsJointOrderPreparation, PhysicsJointPositionSchedule,
        PhysicsJointPositionScope,
    },
};
use avian3d::dynamics::integrator::IntegrationSystems;
use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
use avian3d::dynamics::solver::{
    schedule::SolverSystems,
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{PositionConstraint, XpbdSolverSystems},
};
#[cfg(test)]
use avian3d::parry::bounding_volume::BoundingVolume;
use avian3d::parry::query::details::{NormalConstraints, NormalConstraintsPair};
use avian3d::parry::{
    bounding_volume::Aabb,
    math::Pose,
    query::{
        NonlinearRigidMotion, ShapeCastHit, ShapeCastOptions, ShapeCastStatus, cast_shapes,
        cast_shapes_nonlinear,
    },
};
use avian3d::prelude::*;
use bevy::{
    ecs::system::StaticSystemParam,
    math::{DMat3, DQuat, DVec3},
    platform::collections::HashMap,
    prelude::*,
    tasks::{ComputeTaskPool, ParallelSliceMut},
};
use std::marker::PhantomData;

/// Deterministic convergence bounds for continuous contact and joint constraints.
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct ContinuousCollisionSettings {
    /// Minimum candidate queries per scoped worker. Smaller populations run
    /// directly; zero permits one query per worker. Changes work assignment
    /// only, with every result consumed in prepared pair order.
    pub minimum_query_batch_size: usize,
    /// Maximum position iterations per substep; failure holds the uncommitted tick.
    pub max_iterations: usize,
    /// Maximum remaining contact correction in metres.
    pub position_tolerance_m: f64,
    /// Maximum remaining joint rotation correction in radians.
    pub angular_tolerance_rad: f64,
}
impl Default for ContinuousCollisionSettings {
    fn default() -> Self {
        Self {
            minimum_query_batch_size: 8,
            max_iterations: 1024,
            position_tolerance_m: 1e-6,
            angular_tolerance_rad: 1e-6,
        }
    }
}

/// Non-authoritative counters for the most recent physics step.
#[derive(Resource, Default, Clone, Debug)]
pub struct ContinuousCollisionDiagnostics {
    pub substeps: usize,
    pub sweep_queries: usize,
    pub position_iterations: usize,
    pub solve_time: std::time::Duration,
    pub sweep_time: std::time::Duration,
    pub joint_time: std::time::Duration,
}

/// Installs continuous inequalities before XPBD velocity projection, with the
/// same pair filtering as the native broad phase. Native body-only CCD is disabled.
pub struct ContinuousCollisionPlugin<H: CollisionHooks = ()>(PhantomData<H>);
impl<H: CollisionHooks> Default for ContinuousCollisionPlugin<H> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<H: CollisionHooks + 'static> Plugin for ContinuousCollisionPlugin<H>
where
    for<'w, 's> bevy::ecs::system::SystemParamItem<'w, 's, H>: CollisionHooks,
{
    fn build(&self, app: &mut App) {
        app.init_resource::<ContinuousCollisionSettings>()
            .register_type::<ContinuousCollisionSettings>()
            .init_scene_resource::<ContinuousCollisionDiagnostics>()
            .init_scene_resource::<ContinuousContacts>()
            .add_systems(
                PhysicsSchedule,
                prepare_pairs::<H>
                    .after(PhysicsJointOrderPreparation)
                    .after(SolverSystems::PrepareContactConstraints)
                    .before(SolverSystems::Substep),
            )
            .add_systems(
                SubstepSchedule,
                capture_start.before(IntegrationSystems::Velocity),
            )
            .add_systems(
                SubstepSchedule,
                solve_contacts
                    .after(XpbdSolverSystems::SolveUserConstraints)
                    .before(XpbdSolverSystems::VelocityProjection),
            );
    }
}
#[derive(Clone)]
struct Shape {
    entity: Entity,
    part_index: usize,
    body: Entity,
    collider: Collider,
    position: DVec3,
    rotation: DQuat,
    center: DVec3,
}
struct SweepPair {
    shapes: [usize; 2],
    ccd: [Option<SweptCcd>; 2],
    mode: SweepMode,
}
#[derive(Resource, Default)]
struct ContinuousContacts {
    shapes: Vec<Shape>,
    pairs: Vec<SweepPair>,
    pair_indices: HashMap<(usize, usize), usize>,
    active_shapes: std::collections::BTreeSet<usize>,
    swept_bvh: avian3d::parry::partitioning::Bvh,
    initial_contacts: HashMap<usize, InitialContactCache>,
    prepared: Vec<Plane>,
    impacts: Vec<Plane>,
    starts: HashMap<Entity, SolverBody>,
    impulses: Vec<ContinuousContactImpulse>,
}
#[derive(Clone)]
struct Plane {
    collider1: Entity,
    collider2: Entity,
    part1: usize,
    part2: usize,
    body1: Entity,
    body2: Entity,
    center1: DVec3,
    center2: DVec3,
    point1: DVec3,
    point2: DVec3,
    normal: DVec3,
    offset: f64,
    native_point: Option<(usize, usize, usize)>,
}
impl PositionConstraint for Plane {}

/// Applied continuous impulses that have not been accounted for by a native
/// prepared contact. Exact collider identities retain the pair query's scope.
#[derive(Clone)]
pub(crate) struct ContinuousContactImpulse {
    pub collider1: Entity,
    pub collider2: Entity,
    pub part1: usize,
    pub part2: usize,
    pub impulse: f64,
}
pub(crate) fn continuous_contact_impulses(world: &World) -> Option<&[ContinuousContactImpulse]> {
    Some(&world.get_resource::<ContinuousContacts>()?.impulses)
}

/// Sweep each admitted compound leaf independently. Initial contact by one
/// leaf must not hide a different leaf's new crossing through the same body.
fn shape_leaves(
    shape: &avian3d::parry::shape::SharedShape,
    position: DVec3,
    rotation: DQuat,
    out: &mut Vec<(Collider, DVec3, DQuat)>,
) {
    if let Some(compound) = shape.as_compound() {
        for (pose, child) in compound.shapes() {
            shape_leaves(
                child,
                position + rotation * pose.translation,
                rotation * pose.rotation,
                out,
            );
        }
    } else {
        out.push((Collider::from(shape.clone()), position, rotation));
    }
}

#[allow(clippy::type_complexity)]
fn prepare_pairs<H: CollisionHooks>(
    shapes: Query<
        (
            Entity,
            &Collider,
            &ColliderOf,
            &Position,
            &Rotation,
            Option<&CollisionLayers>,
            Option<&ActiveCollisionHooks>,
            Option<&crate::PhysicsOrderKey>,
        ),
        (Without<Sensor>, Without<ColliderDisabled>),
    >,
    bodies: Query<
        (
            &RigidBody,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            Option<&SweptCcd>,
        ),
        Without<RigidBodyDisabled>,
    >,
    body_facts: Query<
        (
            Entity,
            Option<&crate::PhysicsOrderKey>,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            &ComputedMass,
            &ComputedAngularInertia,
            &LinearVelocity,
            &AngularVelocity,
        ),
        With<RigidBody>,
    >,
    physics_time: Res<Time<Physics>>,
    constraint_graph: Res<ConstraintGraph>,
    contact_graph: Res<ContactGraph>,
    hooks: StaticSystemParam<H>,
    mut commands: Commands,
    mut contacts: ResMut<ContinuousContacts>,
    mut diagnostics: ResMut<ContinuousCollisionDiagnostics>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut holds: ResMut<PhysicsHolds>,
) where
    for<'w, 's> bevy::ecs::system::SystemParamItem<'w, 's, H>: CollisionHooks,
{
    contacts.pairs.clear();
    contacts.shapes.clear();
    contacts.pair_indices.clear();
    contacts.initial_contacts.clear();
    contacts.active_shapes.clear();
    contacts.swept_bvh = avian3d::parry::partitioning::Bvh::new();
    contacts.prepared.clear();
    contacts.impacts.clear();
    contacts.impulses.clear();
    *diagnostics = ContinuousCollisionDiagnostics::default();
    let order = crate::ordered_physics_entities(
        shapes
            .iter()
            .map(|(entity, _, _, _, _, _, _, key)| (entity, key)),
        "continuous collider pairs",
    );
    let order = match order {
        Ok(order) => order,
        Err(invalid) => {
            crate::report_invalid_physics_order(
                Some(&mut holds),
                Some(&mut faults),
                None,
                "physics-continuous-order-invalid",
                &invalid,
            );
            return;
        }
    };
    let ranks = order
        .iter()
        .enumerate()
        .map(|(i, e)| (*e, i))
        .collect::<HashMap<_, _>>();
    let all = order
        .into_iter()
        .map(|entity| shapes.get(entity).expect("validated collider identity"))
        .collect::<Vec<_>>();
    let audit = bevy::log::debug_span!(target: "lunco_physics::order_audit", "physics_order_audit");
    if !audit.is_disabled() {
        let _audit = audit.enter();
        let mut bodies = body_facts.iter().collect::<Vec<_>>();
        bodies.sort_by(|left, right| left.1.map(|key| &key.0).cmp(&right.1.map(|key| &key.0)));
        bevy::log::debug!(target: "lunco_physics::order_audit",
            "PHYSICS_ORDER_BODIES time={:?} bodies={bodies:?}", physics_time.elapsed_secs_f64());
        let colliders = all
            .iter()
            .map(|(entity, _, owner, position, rotation, _, _, key)| {
                (
                    key.map(|key| &key.0),
                    entity,
                    owner.body,
                    position.0,
                    rotation.0,
                )
            })
            .collect::<Vec<_>>();
        bevy::log::debug!(target: "lunco_physics::order_audit",
            "PHYSICS_ORDER_COLLIDERS time={:?} colliders={colliders:?}", physics_time.elapsed_secs_f64());
        for (entity, collider, _, _, _, _, _, key) in &all {
            if let Some(compound) = collider.shape_scaled().as_compound() {
                let parts = compound
                    .shapes()
                    .iter()
                    .map(|(pose, shape)| (pose, shape.mass_properties(1.0)))
                    .collect::<Vec<_>>();
                bevy::log::debug!(target: "lunco_physics::order_audit",
                    "PHYSICS_ORDER_COMPOUND time={:?} entity={entity:?} key={:?} scale={:?} parts={parts:?}",
                    physics_time.elapsed_secs_f64(), key.map(|key| &key.0), collider.scale());
            }
        }
        for (color, group) in constraint_graph.colors.iter().enumerate() {
            for (index, constraint) in group.contact_constraints.iter().enumerate() {
                let pair = contact_graph
                    .get_by_id(constraint.contact_id)
                    .map(|(_, pair)| pair);
                bevy::log::debug!(target: "lunco_physics::order_audit",
                    "PHYSICS_ORDER_CONTACT time={:?} color={color} index={index} id={:?} pair={:?} manifold={} normal={:?} points={:?}",
                    physics_time.elapsed_secs_f64(), constraint.contact_id,
                    pair.map(|pair| (pair.collider1, pair.collider2, pair.body1, pair.body2)),
                    constraint.manifold_index, constraint.normal, constraint.points);
            }
        }
    }
    let leaves = all
        .iter()
        .map(|(_, collider, _, position, rotation, _, _, _)| {
            let mut leaves = Vec::new();
            shape_leaves(collider.shape_scaled(), position.0, rotation.0, &mut leaves);
            leaves
        })
        .collect::<Vec<_>>();
    let mut prepared = Vec::new();
    for (color_index, color) in constraint_graph.colors.iter().enumerate() {
        for (constraint_index, constraint) in color.contact_constraints.iter().enumerate() {
            let Some((_, pair)) = contact_graph.get_by_id(constraint.contact_id) else {
                continue;
            };
            let (Some(rank1), Some(rank2)) =
                (ranks.get(&pair.collider1), ranks.get(&pair.collider2))
            else {
                continue;
            };
            let Ok([(k1, p1, r1, com1, ccd1), (k2, p2, r2, com2, ccd2)]) =
                bodies.get_many([constraint.body1, constraint.body2])
            else {
                continue;
            };
            if !ccd1.is_some_and(|c| !k2.is_dynamic() || c.include_dynamic)
                && !ccd2.is_some_and(|c| !k1.is_dynamic() || c.include_dynamic)
            {
                continue;
            }
            let center1 = p1.0 + r1.0 * com1.0;
            let center2 = p2.0 + r2.0 * com2.0;
            for (point_index, point) in constraint.points.iter().enumerate() {
                let mut plane = Plane {
                    collider1: pair.collider1,
                    collider2: pair.collider2,
                    part1: 0,
                    part2: 0,
                    body1: constraint.body1,
                    body2: constraint.body2,
                    center1,
                    center2,
                    point1: point.anchor1,
                    point2: point.anchor2,
                    normal: constraint.normal,
                    offset: point.initial_separation - (center2 - center1).dot(constraint.normal),
                    native_point: Some((color_index, constraint_index, point_index)),
                };
                if rank1 > rank2 {
                    std::mem::swap(&mut plane.collider1, &mut plane.collider2);
                    std::mem::swap(&mut plane.body1, &mut plane.body2);
                    std::mem::swap(&mut plane.center1, &mut plane.center2);
                    std::mem::swap(&mut plane.point1, &mut plane.point2);
                    plane.normal = -plane.normal;
                }
                prepared.push((
                    (
                        *rank1.min(rank2),
                        *rank1.max(rank2),
                        constraint.manifold_index,
                        point_index,
                    ),
                    plane,
                ));
            }
        }
    }
    prepared.sort_by_key(|(key, _)| *key);
    contacts
        .prepared
        .extend(prepared.into_iter().map(|(_, plane)| plane));
    // Static terrain/scatter populations must not create a quadratic pair set.
    // Generate pairs from CCD owners, then canonicalize their stable indices.
    let active = all
        .iter()
        .enumerate()
        .filter_map(|(i, (_, _, owner, _, _, _, _, _))| {
            bodies
                .get(owner.body)
                .ok()
                .and_then(|(_, _, _, _, ccd)| ccd.map(|_| i))
        })
        .collect::<Vec<_>>();
    let mut pair_indices = std::collections::BTreeSet::new();
    for i in active {
        for j in 0..all.len() {
            if i != j {
                pair_indices.insert((i.min(j), i.max(j)));
            }
        }
    }
    let mut shape_indices = HashMap::new();
    for (i, j) in pair_indices {
        let (e1, _, o1, _, _, l1, h1, _) = all[i];
        let (e2, _, o2, _, _, l2, h2, _) = all[j];
        if o1.body == o2.body
            || !l1
                .copied()
                .unwrap_or_default()
                .interacts_with(l2.copied().unwrap_or_default())
        {
            continue;
        }
        let Ok([(k1, bp1, br1, com1, ccd1), (k2, bp2, br2, com2, ccd2)]) =
            bodies.get_many([o1.body, o2.body])
        else {
            continue;
        };
        let ccd1 = ccd1.filter(|ccd| !k2.is_dynamic() || ccd.include_dynamic);
        let ccd2 = ccd2.filter(|ccd| !k1.is_dynamic() || ccd.include_dynamic);
        if ccd1.is_none() && ccd2.is_none() {
            continue;
        }
        if [h1, h2]
            .into_iter()
            .flatten()
            .any(|h| h.contains(ActiveCollisionHooks::FILTER_PAIRS))
            && !hooks.filter_pairs(e1, e2, &mut commands)
        {
            continue;
        }
        let mode = if [ccd1, ccd2]
            .into_iter()
            .flatten()
            .any(|c| c.mode == SweepMode::NonLinear)
        {
            SweepMode::NonLinear
        } else {
            SweepMode::Linear
        };
        for (part1, (collider1, position1, rotation1)) in leaves[i].iter().enumerate() {
            for (part2, (collider2, position2, rotation2)) in leaves[j].iter().enumerate() {
                let mut intern =
                    |entity, part_index, body, collider: &Collider, position, rotation, center| {
                        *shape_indices
                            .entry((entity, part_index))
                            .or_insert_with(|| {
                                let index = contacts.shapes.len();
                                contacts.shapes.push(Shape {
                                    entity,
                                    part_index,
                                    body,
                                    collider: collider.clone(),
                                    position,
                                    rotation,
                                    center,
                                });
                                index
                            })
                    };
                let indices = [
                    intern(
                        e1,
                        part1,
                        o1.body,
                        collider1,
                        *position1,
                        *rotation1,
                        bp1.0 + br1.0 * com1.0,
                    ),
                    intern(
                        e2,
                        part2,
                        o2.body,
                        collider2,
                        *position2,
                        *rotation2,
                        bp2.0 + br2.0 * com2.0,
                    ),
                ];
                let ccd = [ccd1.copied(), ccd2.copied()];
                for (index, ccd) in indices.into_iter().zip(ccd) {
                    if ccd.is_some() {
                        contacts.active_shapes.insert(index);
                    }
                }
                let pair_index = contacts.pairs.len();
                contacts.pair_indices.insert(
                    (indices[0].min(indices[1]), indices[0].max(indices[1])),
                    pair_index,
                );
                contacts.pairs.push(SweepPair {
                    shapes: indices,
                    ccd,
                    mode,
                });
            }
        }
    }
}
fn capture_start(bodies: Query<(Entity, &SolverBody)>, mut contacts: ResMut<ContinuousContacts>) {
    for cache in contacts.initial_contacts.values_mut() {
        cache.advance_substep();
    }
    contacts.starts.clear();
    contacts
        .starts
        .extend(bodies.iter().map(|(e, b)| (e, b.clone())));
}
struct BodyMotion {
    start_position: DVec3,
    start_rotation: DQuat,
    linear: DVec3,
    angular: DVec3,
}
impl BodyMotion {
    fn new(start: &SolverBody, end: &SolverBody) -> Result<Self, String> {
        Ok(Self {
            start_position: start.delta_position,
            start_rotation: start.delta_rotation.0,
            linear: end.delta_position - start.delta_position,
            angular: rotation_displacement(start.delta_rotation.0, end.delta_rotation.0)?,
        })
    }
}
fn motion(shape: &Shape, body: &BodyMotion) -> NonlinearRigidMotion {
    let center_local = shape.rotation.inverse() * (shape.center - shape.position);
    let pose = Pose::from_parts(
        shape.center + body.start_position + body.start_rotation * (shape.position - shape.center),
        body.start_rotation * shape.rotation,
    );
    NonlinearRigidMotion::new(pose, center_local, body.linear, body.angular)
}
/// Every point follows a linear COM trajectory plus a rotational arc. The
/// endpoint chord's maximum deviation is bounded by |p''| / 8, so expanding
/// the endpoint AABBs by this remainder encloses the complete swept shape.
fn swept_bounds(shape: &Shape, motion: &NonlinearRigidMotion) -> Aabb {
    let sphere = shape
        .collider
        .shape_scaled()
        .compute_local_bounding_sphere();
    let radius = sphere.radius + (sphere.center - motion.local_center).length();
    let start = shape.collider.shape_scaled().compute_aabb(&motion.start);
    let end = shape
        .collider
        .shape_scaled()
        .compute_aabb(&motion.position_at_time(1.0));
    let remainder = DVec3::splat(radius * motion.angvel.length_squared() / 8.0);
    Aabb::new(
        start.mins.min(end.mins) - remainder,
        start.maxs.max(end.maxs) + remainder,
    )
}

/// Immutable native inputs shared by candidate-axis tests for one leaf sweep.
/// Changed motion or geometry constructs a new interval before any certificate.
struct SupportInterval<'a> {
    motions: [&'a NonlinearRigidMotion; 2],
    shapes: [&'a dyn avian3d::parry::shape::Shape; 2],
    supports: [&'a dyn avian3d::parry::shape::SupportMap; 2],
    spheres: [avian3d::parry::bounding_volume::BoundingSphere; 2],
    poses: [[Pose; 2]; 2],
    relative: [Pose; 2],
}
impl<'a> SupportInterval<'a> {
    fn new(
        m1: &'a NonlinearRigidMotion,
        s1: &'a dyn avian3d::parry::shape::Shape,
        m2: &'a NonlinearRigidMotion,
        s2: &'a dyn avian3d::parry::shape::Shape,
    ) -> Option<Self> {
        let (Some(support1), Some(support2)) = (s1.as_support_map(), s2.as_support_map()) else {
            return None;
        };
        let poses = [
            [m1.position_at_time(0.0), m1.position_at_time(1.0)],
            [m2.position_at_time(0.0), m2.position_at_time(1.0)],
        ];
        Some(Self {
            motions: [m1, m2],
            shapes: [s1, s2],
            supports: [support1, support2],
            spheres: [
                s1.compute_local_bounding_sphere(),
                s2.compute_local_bounding_sphere(),
            ],
            relative: [
                poses[0][0].inv_mul(&poses[1][0]),
                poses[0][1].inv_mul(&poses[1][1]),
            ],
            poses,
        })
    }
    /// Certify the full-shape gap throughout the actual COM arc, including
    /// rotation and changing support features, along a fixed world axis.
    fn gap_bound(&self, axis: DVec3) -> Result<(f64, f64), String> {
        let _span =
            bevy::log::debug_span!(target: "lunco_physics::ccd::support", "ccd_support_gap_bound")
                .entered();
        let acceleration = |index: usize| {
            let shape = self.shapes[index];
            let motion = self.motions[index];
            let sphere = self.spheres[index];
            let offset = motion.start.rotation * (sphere.center - motion.local_center);
            let center = motion.angvel.cross(offset).length() * motion.angvel.cross(axis).length();
            let geometry = if shape.as_ball().is_some() {
                0.0
            } else if shape.as_cylinder().is_some() {
                // Remove axial spin by parallel-transporting the cylinder's
                // material correspondence. Its occupied geometry is unchanged.
                // k = omega - a*(omega dot a), |k'| = |omega cross a|*|omega dot a|.
                // Thus |p''| <= radius*(|k'|+|k|^2), including changing tilt.
                let cylinder_axis = (motion.start.rotation * DVec3::Y).normalize();
                let perpendicular = motion.angvel.cross(cylinder_axis).length();
                sphere.radius
                    * perpendicular
                    * (perpendicular + motion.angvel.dot(cylinder_axis).abs())
            } else {
                sphere.radius * motion.angvel.length() * motion.angvel.cross(axis).length()
            };
            center + geometry
        };
        let acceleration =
            ((acceleration(0) + acceleration(1)) * (1.0 + 64.0 * f64::EPSILON)).next_up();
        let mut initial = 0.0;
        let mut lower = f64::INFINITY;
        for index in 0..2 {
            // Subtract native pose origins before adding occupied geometry. A
            // common world translation must not erase a small contact clearance.
            let local_axis = self.poses[0][index].rotation.inverse() * axis;
            let point1 = self.supports[0].local_support_point(local_axis);
            let point2 = self.supports[1]
                .local_support_point(self.relative[index].rotation.inverse() * -local_axis);
            let max1 = point1.dot(local_axis);
            let min2 = (self.relative[index] * point2).dot(local_axis);
            let gap = min2 - max1;
            if !gap.is_finite() {
                return Err(
                    "continuous separating-axis support exceeded its finite numeric range".into(),
                );
            }
            if index == 0 {
                initial = gap;
            }
            // Bound rotation, support projection and subtraction arithmetic in
            // this relative frame. The allowance is numerical uncertainty, not
            // additional contact tolerance or a retained clear-space result.
            let scale = self.relative[index].translation.abs().element_sum()
                + point1.abs().element_sum()
                + point2.abs().element_sum();
            let roundoff = (64.0 * f64::EPSILON * scale * local_axis.abs().element_sum()).next_up();
            let bound = (gap - roundoff).next_down();
            if !roundoff.is_finite() || !bound.is_finite() {
                return Err(
                    "continuous relative support arithmetic exceeded its finite numeric range"
                        .into(),
                );
            }
            lower = lower.min(bound);
        }
        if !acceleration.is_finite() || !initial.is_finite() || !lower.is_finite() {
            return Err(
                "continuous separating-axis bound exceeded its finite numeric range".into(),
            );
        }
        // Every material-point pair follows a chord with this projected
        // second-derivative remainder. Supports include all feature changes.
        Ok((initial, (lower - acceleration / 8.0).next_down()))
    }
}
fn triangle_separation_axis(
    interval: &SupportInterval,
    tolerance: f64,
) -> Result<Option<DVec3>, String> {
    let [m1, m2] = interval.motions;
    let [s1, s2] = interval.shapes;
    let axis = if let Some(triangle) = s1.as_triangle() {
        triangle.normal().map(|n| m1.start.rotation * n)
    } else if let Some(triangle) = s2.as_triangle() {
        triangle.normal().map(|n| m2.start.rotation * n)
    } else {
        None
    };
    let Some(axis) = axis else {
        return Ok(None);
    };
    for direction in [axis, -axis] {
        if interval.gap_bound(direction)?.1 >= -tolerance {
            return Ok(Some(direction));
        }
    }
    Ok(None)
}
/// Immutable starting geometry is shared by all closure sweeps in one substep.
/// Native composite part identities isolate each leaf query.
#[derive(Default)]
struct InitialContactCache {
    candidate_axis: Option<DVec3>,
    depth_axis: Option<Option<DepthAxis>>,
    contact: Option<avian3d::parry::query::Contact>,
    parts: HashMap<u32, InitialContactCache>,
}
impl InitialContactCache {
    /// Only an advisory world direction survives a changed starting pose.
    /// It supplies no geometry or clearance; every use evaluates fresh supports.
    fn advance_substep(&mut self) {
        self.depth_axis = None;
        self.contact = None;
        for part in self.parts.values_mut() {
            part.advance_substep();
        }
    }
}

/// Reconstruct a real convex-combination witness from native feature location.
/// Valid positive weights establish triangle membership independently of the
/// conditioning of its closest-point calculation. Uncertain locations cannot
/// supply a certificate; nonfinite native data remains a query error.
fn triangle_projection_witness(
    triangle: &avian3d::parry::shape::Triangle,
    point: DVec3,
) -> Result<Option<DVec3>, String> {
    use avian3d::parry::query::PointQueryWithLocation;
    let (_, location) = triangle.project_local_point_and_get_location(point, true);
    let Some(weights) = location.barycentric_coordinates() else {
        return Ok(None);
    };
    let weights = DVec3::from_array(weights);
    if !weights.is_finite() {
        return Err("native triangle location returned nonfinite barycentric geometry".into());
    }
    if weights.min_element() < 0.0 {
        return Ok(None);
    }
    let sum = weights.element_sum();
    if !sum.is_finite() || sum <= 0.0 {
        return Err("native triangle location has no finite positive barycentric weight".into());
    }
    let weights = weights / sum;
    let witness = triangle.a * weights.x + triangle.b * weights.y + triangle.c * weights.z;
    if !witness.is_finite() {
        return Err("native triangle witness exceeded its finite numeric range".into());
    }
    Ok(Some(witness))
}

fn cylinder_interior_radius(
    cylinder: &avian3d::parry::shape::Cylinder,
    point: DVec3,
    geometry_scale: f64,
) -> Result<Option<f64>, String> {
    use avian3d::parry::query::PointQuery;
    if !point.is_finite() || !geometry_scale.is_finite() {
        return Err("native cylinder interior witness exceeded its finite numeric range".into());
    }
    let projection = cylinder.project_local_point(point, false);
    if !projection.point.is_finite() {
        return Err("native cylinder point projection returned nonfinite geometry".into());
    }
    if !projection.is_inside {
        return Ok(None);
    }
    let radius = (projection.point - point).length();
    let roundoff = 128.0
        * f64::EPSILON
        * (geometry_scale + point.abs().element_sum() + cylinder.radius + cylinder.half_height);
    if !radius.is_finite() || !roundoff.is_finite() {
        return Err("finite triangle interior depth exceeded its finite numeric range".into());
    }
    Ok(Some((radius - roundoff).max(0.0)))
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct DepthAxis {
    axis: DVec3,
    lower_depth: f64,
}

struct SweepJob {
    pair_index: usize,
    cache: InitialContactCache,
}

struct SweepResult {
    hits: Result<Vec<ShapeCastHit>, String>,
    queries: usize,
}

/// Evaluate immutable candidate geometry in scoped native workers. Each worker
/// owns its cache; the caller consumes the joined results in
/// prepared pair order before applying any authoritative correction.
fn query_sweep_jobs(
    shapes: &[Shape],
    pairs: &[SweepPair],
    trajectories: &[(NonlinearRigidMotion, Aabb)],
    mut jobs: &mut [SweepJob],
    tolerance: f64,
    minimum_batch_size: usize,
    pool: &bevy::tasks::TaskPool,
) -> Vec<SweepResult> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let workers = pool
        .thread_num()
        .max(1)
        .min((jobs.len() / minimum_batch_size.max(1)).max(1));
    let _span =
        bevy::log::debug_span!("ccd_query_dispatch", queries = jobs.len(), workers).entered();
    let evaluate_chunk = |jobs: &mut [SweepJob]| {
        let _span = bevy::log::debug_span!("ccd_query_chunk", queries = jobs.len()).entered();
        jobs.iter_mut()
            .map(|job| {
                let pair = &pairs[job.pair_index];
                let a = &shapes[pair.shapes[0]];
                let b = &shapes[pair.shapes[1]];
                let (m1, bounds1) = trajectories[pair.shapes[0]];
                let (m2, bounds2) = trajectories[pair.shapes[1]];
                let mut queries = 0;
                let hits = shape_hits(
                    pair.mode,
                    &m1,
                    a.collider.shape_scaled().as_ref(),
                    &bounds1,
                    None,
                    &m2,
                    b.collider.shape_scaled().as_ref(),
                    &bounds2,
                    None,
                    &mut queries,
                    tolerance,
                    &mut job.cache,
                )
                .map_err(|detail| {
                    format!(
                        "continuous sweep {:?}[{}] / {:?}[{}]: {detail}",
                        a.entity, a.part_index, b.entity, b.part_index,
                    )
                });
                SweepResult { hits, queries }
            })
            .collect::<Vec<_>>()
    };
    if workers == 1 {
        return evaluate_chunk(jobs);
    }
    let chunk_size = jobs.len().div_ceil(workers);
    jobs.par_chunk_map_mut(pool, chunk_size, |_, jobs| evaluate_chunk(jobs))
        .into_iter()
        .flatten()
        .collect()
}

/// Certify a finite triangle's initial minimum translation depth from native
/// supports and finite features. An interior ball at a real triangle point or
/// whole-projection clearance at all three edges proves a lower bound; a
/// matching support depth proves separation. A plane overlap alone cannot.
fn finite_triangle_depth_axis(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    tolerance: f64,
) -> Result<Option<DepthAxis>, String> {
    if s1.as_triangle().is_none() && s2.as_triangle().is_some() {
        return finite_triangle_depth_axis(m2, s2, m1, s1, tolerance).map(|axis| {
            axis.map(|axis| DepthAxis {
                axis: -axis.axis,
                ..axis
            })
        });
    }
    let (Some(triangle), Some(support)) = (s1.as_triangle(), s2.as_support_map()) else {
        return Ok(None);
    };
    let Some(normal) = triangle.normal() else {
        return Ok(None);
    };
    let relative = m1.start.inv_mul(&m2.start);
    if let Some(cylinder) = s2.as_cylinder() {
        use avian3d::parry::shape::SupportMap;
        // A real triangle point at distance r from the cylinder boundary owns
        // an interior ball of radius r. Every translation shorter than r still
        // intersects that point. A matching support depth supplies the upper
        // bound, proving the MTD without replacing finite features by a plane.
        let cap_axis = relative.rotation * DVec3::Y;
        for axis in [normal, -normal, cap_axis, -cap_axis] {
            let upper_point = triangle.local_support_point(axis);
            let gap = (support.support_point(&relative, -axis) - upper_point).dot(axis);
            if !gap.is_finite() {
                return Err(
                    "finite triangle support depth exceeded its finite numeric range".into(),
                );
            }
            if gap > 0.0 {
                continue;
            }
            let center_projection = triangle_projection_witness(triangle, relative.translation)?;
            for point in [Some(upper_point), center_projection].into_iter().flatten() {
                let local_point = relative.inverse_transform_point(point);
                let scale = triangle.a.abs().element_sum()
                    + triangle.b.abs().element_sum()
                    + triangle.c.abs().element_sum()
                    + m1.start.translation.abs().element_sum()
                    + m2.start.translation.abs().element_sum();
                let Some(lower_depth) = cylinder_interior_radius(cylinder, local_point, scale)?
                else {
                    continue;
                };
                if lower_depth >= -gap - tolerance {
                    return Ok(Some(DepthAxis {
                        axis: m1.start.rotation * axis,
                        lower_depth,
                    }));
                }
            }
        }
    }
    let gap = (support.support_point(&relative, -normal) - triangle.a).dot(normal);
    let reverse_gap = (support.support_point(&relative, normal) - triangle.a).dot(-normal);
    if !gap.is_finite() || !reverse_gap.is_finite() {
        return Err("finite triangle contact bound exceeded its finite numeric range".into());
    }
    // Only initial intersection requires a penetration-depth certificate.
    if gap > 0.0 || reverse_gap > 0.0 {
        return Ok(None);
    }
    let depth = -gap.max(reverse_gap);
    for (a, b) in [
        (triangle.a, triangle.b),
        (triangle.b, triangle.c),
        (triangle.c, triangle.a),
    ] {
        let Some(inward) = normal.cross(b - a).try_normalize() else {
            return Ok(None);
        };
        let clearance = (support.support_point(&relative, -inward) - a).dot(inward);
        if !clearance.is_finite() {
            return Err("finite triangle edge bound exceeded its finite numeric range".into());
        }
        if clearance < depth + tolerance {
            return Ok(None);
        }
    }
    Ok(Some(DepthAxis {
        axis: m1.start.rotation * if gap >= reverse_gap { normal } else { -normal },
        lower_depth: depth,
    }))
}

/// Certify the native interval with whole-arc supports while retaining each
/// convex leaf's finite-feature geometry.
fn certified_native_interval(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    tolerance: f64,
    cache: &mut InitialContactCache,
) -> Result<bool, String> {
    if s1.as_support_map().is_none() || s2.as_support_map().is_none() {
        return Ok(false);
    }
    let Some(interval) = SupportInterval::new(m1, s1, m2, s2) else {
        return Ok(false);
    };
    if let Some(axis) = cache.candidate_axis {
        if interval.gap_bound(axis)?.1 >= -tolerance {
            return Ok(true);
        }
        cache.candidate_axis = None;
    }
    if let Some(Some(axis)) = cache.depth_axis {
        return Ok(interval.gap_bound(axis.axis)?.1 >= -axis.lower_depth - tolerance);
    }
    if let Some(contact) = cache.contact {
        let certified = native_contact_axis_clear(&interval, &contact, tolerance)?;
        if certified || contact.dist < -tolerance {
            // A minimum translation depth beyond tolerance proves that every
            // separating-axis support depth also exceeds tolerance at t=0.
            // Other axes cannot certify a clear whole arc; do not repeat them.
            return Ok(certified);
        }
    }
    if let Some(axis) = triangle_separation_axis(&interval, tolerance)? {
        cache.candidate_axis = Some(axis);
        return Ok(true);
    }
    let [sphere1, sphere2] = interval.spheres;
    let center1 = m1.start * sphere1.center;
    let center2 = m2.start * sphere2.center;
    // Native finite-triangle projection supplies a cheap candidate edge axis.
    // Only the full-shape whole-arc support bound can certify separation.
    use avian3d::parry::query::PointQuery;
    let axis = if let Some(triangle) = s1.as_triangle() {
        center2 - triangle.project_point(&m1.start, center2, true).point
    } else if let Some(triangle) = s2.as_triangle() {
        triangle.project_point(&m2.start, center1, true).point - center1
    } else {
        center2 - center1
    };
    // A cylinder's radial feature supplies a second candidate independent of
    // cap displacement. The occupied shape, not an enclosing sphere or an
    // assumed closest point, still certifies the complete arc on that axis.
    let cylinder_axis = if s1.as_cylinder().is_some() && s2.as_triangle().is_some() {
        Some(m1.start.rotation * DVec3::Y)
    } else if s2.as_cylinder().is_some() && s1.as_triangle().is_some() {
        Some(m2.start.rotation * DVec3::Y)
    } else {
        None
    };
    let radial = cylinder_axis.and_then(|cap_axis| {
        let component = axis.dot(cap_axis);
        (component != 0.0).then_some(axis - cap_axis * component)
    });
    for axis in [Some(axis), radial].into_iter().flatten() {
        if axis.is_finite() && axis.length_squared() > 0.0 {
            let unit = axis.normalize();
            if interval.gap_bound(unit)?.1 >= -tolerance {
                cache.candidate_axis = Some(unit);
                return Ok(true);
            }
        }
    }
    let depth_axis = match cache.depth_axis {
        Some(axis) => axis,
        None => {
            let axis = finite_triangle_depth_axis(m1, s1, m2, s2, tolerance)?;
            cache.depth_axis = Some(axis);
            axis
        }
    };
    if let Some(axis) = depth_axis {
        return Ok(interval.gap_bound(axis.axis)?.1 >= -axis.lower_depth - tolerance);
    }
    let prediction = (m1.start * sphere1.center - m2.start * sphere2.center).length()
        + sphere1.radius
        + sphere2.radius;
    if !prediction.is_finite() {
        return Err("continuous native-axis range exceeded its finite numeric range".into());
    }
    let contact = match cache.contact {
        Some(contact) => contact,
        None => {
            let contact = initial_native_contact(m1, s1, m2, s2, prediction, axis)?;
            cache.contact = Some(contact);
            contact
        }
    };
    let certified = native_contact_axis_clear(&interval, &contact, tolerance)?;
    if certified && contact.dist > 0.0 {
        cache.candidate_axis = Some(contact.normal1);
    }
    Ok(certified)
}

fn native_contact_axis_clear(
    interval: &SupportInterval,
    contact: &avian3d::parry::query::Contact,
    tolerance: f64,
) -> Result<bool, String> {
    let (initial, lower) = interval.gap_bound(contact.normal1)?;
    if lower >= -tolerance {
        return Ok(true);
    }
    if contact.dist > 0.0 {
        return Ok(false);
    }
    // A native contact in world space provides the separating direction.
    // Require its depth to agree with the full-shape support depth, rather
    // than admitting a deeper guessed baseline at an edge contact.
    Ok((initial - contact.dist).abs() <= tolerance && lower >= initial.min(0.0) - tolerance)
}

/// Seed Parry's native triangle/support-map query with the finite-feature axis
/// already computed for this geometry. GJK/EPA still owns the contact depth;
/// the seed supplies no constraint or assumed infinite-plane penetration.
/// The caller's enclosing prediction requires geometry even for separated
/// shapes. Native missing geometry is an incomplete query, never clear space.
fn initial_native_contact(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    prediction: f64,
    axis: DVec3,
) -> Result<avian3d::parry::query::Contact, String> {
    let _span = bevy::log::debug_span!("ccd_initial_native_contact", shape1=?s1.shape_type(), shape2=?s2.shape_type()).entered();
    use avian3d::parry::query::{
        Contact,
        details::contact_support_map_support_map_with_params,
        gjk::{GJKResult, VoronoiSimplex},
    };
    let missing_geometry = |result: &str| {
        format!(
            "native continuous initial contact did not return contact geometry for its enclosing prediction: result={result}, prediction={prediction:?}, axis={axis:?}, types={:?}/{:?}, pose1={:?}, pose2={:?}, triangle1={:?}, triangle2={:?}, ball1={:?}, ball2={:?}, cylinder1={:?}, cylinder2={:?}, cuboid1={:?}, cuboid2={:?}",
            s1.shape_type(),
            s2.shape_type(),
            m1.start,
            m2.start,
            s1.as_triangle(),
            s2.as_triangle(),
            s1.as_ball(),
            s2.as_ball(),
            s1.as_cylinder(),
            s2.as_cylinder(),
            s1.as_cuboid(),
            s2.as_cuboid(),
        )
    };
    // Balls have a specialized native kernel. All other convex triangle pairs
    // use this same support-map kernel in the native default dispatcher.
    let contact = if (s1.as_triangle().is_some() || s2.as_triangle().is_some())
        && s1.as_ball().is_none()
        && s2.as_ball().is_none()
        && let (Some(support1), Some(support2)) = (s1.as_support_map(), s2.as_support_map())
    {
        let relative = m1.start.inv_mul(&m2.start);
        let direction = (axis.is_finite() && axis.length_squared() > 0.0)
            .then(|| m1.start.rotation.inverse() * axis.normalize());
        // Native primitive references let Rust specialize the same GJK/EPA
        // kernel without virtual support calls inside every iteration.
        macro_rules! native_contact {
            ($first:expr, $second:expr) => {
                contact_support_map_support_map_with_params(
                    &relative,
                    $first,
                    $second,
                    prediction,
                    &mut VoronoiSimplex::new(),
                    direction,
                )
            };
        }
        let result = if let (Some(a), Some(b)) = (s1.as_triangle(), s2.as_cylinder()) {
            native_contact!(a, b)
        } else if let (Some(a), Some(b)) = (s1.as_cylinder(), s2.as_triangle()) {
            native_contact!(a, b)
        } else if let (Some(a), Some(b)) = (s1.as_triangle(), s2.as_cuboid()) {
            native_contact!(a, b)
        } else if let (Some(a), Some(b)) = (s1.as_cuboid(), s2.as_triangle()) {
            native_contact!(a, b)
        } else {
            native_contact!(support1, support2)
        };
        match result {
            GJKResult::ClosestPoints(point1, point2_1, normal1) => {
                let distance = (point2_1 - point1).dot(normal1);
                let mut contact = Contact::new(
                    point1,
                    relative.inverse_transform_point(point2_1),
                    normal1,
                    relative.rotation.inverse() * -normal1,
                    distance,
                );
                contact.transform_by_mut(&m1.start, &m2.start);
                contact
            }
            result => return Err(missing_geometry(&format!("{result:?}"))),
        }
    } else {
        avian3d::parry::query::contact(&m1.start, s1, &m2.start, s2, prediction)
            .map_err(|error| format!("unsupported continuous initial contact: {error:?}"))?
            .ok_or_else(|| missing_geometry("native dispatcher returned no contact"))?
    };
    validate_native_contact(&contact)?;
    Ok(contact)
}
fn validate_native_contact(contact: &avian3d::parry::query::Contact) -> Result<(), String> {
    if !contact.dist.is_finite()
        || !contact.point1.is_finite()
        || !contact.point2.is_finite()
        || !contact.normal1.is_finite()
        || !contact.normal2.is_finite()
        || (contact.normal1.length_squared() - 1.0).abs() > 1e-6
        || (contact.normal2.length_squared() - 1.0).abs() > 1e-6
    {
        return Err("continuous contact returned invalid native geometry".into());
    }
    Ok(())
}

/// Parry owns the nonlinear cast, bisection and fresh GJK/EPA geometry. Each
/// contact sample retains its native witnesses at the actual queried pose.
/// Native query failures survive Parry's optional-hit return boundary.
#[derive(Default)]
struct CheckedNativeQueries {
    failure: bevy::platform::sync::Mutex<Option<String>>,
    #[cfg(test)]
    contact_queries: std::sync::atomic::AtomicUsize,
}
impl CheckedNativeQueries {
    fn reject(&self, error: String) -> avian3d::parry::query::Unsupported {
        // The dispatcher trait is Sync, but the native cast invokes it serially.
        // A poisoned error collector is rejected when the cast completes.
        if let Ok(mut failure) = self.failure.lock() {
            if failure.is_none() {
                *failure = Some(error);
            }
        }
        avian3d::parry::query::Unsupported
    }
}
impl avian3d::parry::query::QueryDispatcher for CheckedNativeQueries {
    fn intersection_test(
        &self,
        p: &Pose,
        a: &dyn avian3d::parry::shape::Shape,
        b: &dyn avian3d::parry::shape::Shape,
    ) -> Result<bool, avian3d::parry::query::Unsupported> {
        avian3d::parry::query::DefaultQueryDispatcher.intersection_test(p, a, b)
    }
    fn distance(
        &self,
        p: &Pose,
        a: &dyn avian3d::parry::shape::Shape,
        b: &dyn avian3d::parry::shape::Shape,
    ) -> Result<f64, avian3d::parry::query::Unsupported> {
        avian3d::parry::query::DefaultQueryDispatcher.distance(p, a, b)
    }
    fn contact(
        &self,
        p: &Pose,
        a: &dyn avian3d::parry::shape::Shape,
        b: &dyn avian3d::parry::shape::Shape,
        prediction: f64,
    ) -> Result<Option<avian3d::parry::query::Contact>, avian3d::parry::query::Unsupported> {
        #[cfg(test)]
        self.contact_queries
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _span = bevy::log::debug_span!("ccd_cast_native_contact", shape1=?a.shape_type(), shape2=?b.shape_type()).entered();
        let contact = avian3d::parry::query::DefaultQueryDispatcher
            .contact(p, a, b, prediction)
            .map_err(|error| self.reject(format!("unsupported continuous contact: {error:?}")))?;
        if let Some(contact) = &contact {
            validate_native_contact(contact).map_err(|error| self.reject(error))?;
        }
        if prediction == f64::MAX && contact.is_none() {
            return Err(self
                .reject("continuous unbounded native contact query returned no geometry".into()));
        }
        Ok(contact)
    }
    fn closest_points(
        &self,
        p: &Pose,
        a: &dyn avian3d::parry::shape::Shape,
        b: &dyn avian3d::parry::shape::Shape,
        max_dist: f64,
    ) -> Result<avian3d::parry::query::ClosestPoints, avian3d::parry::query::Unsupported> {
        let _span = bevy::log::debug_span!("ccd_cast_native_closest_points", shape1=?a.shape_type(), shape2=?b.shape_type()).entered();
        let points = avian3d::parry::query::DefaultQueryDispatcher
            .closest_points(p, a, b, max_dist)
            .map_err(|error| {
                self.reject(format!("unsupported continuous closest points: {error:?}"))
            })?;
        if max_dist == f64::MAX && points == avian3d::parry::query::ClosestPoints::Disjoint {
            return Err(self.reject(
                "continuous unbounded native closest-points query returned no geometry".into(),
            ));
        }
        if let avian3d::parry::query::ClosestPoints::WithinMargin(point1, point2) = points {
            if !point1.is_finite() || !point2.is_finite() {
                return Err(self.reject(
                    "continuous closest-points query returned invalid native geometry".into(),
                ));
            }
        }
        Ok(points)
    }
    fn cast_shapes(
        &self,
        p: &Pose,
        velocity: DVec3,
        a: &dyn avian3d::parry::shape::Shape,
        b: &dyn avian3d::parry::shape::Shape,
        options: ShapeCastOptions,
    ) -> Result<Option<ShapeCastHit>, avian3d::parry::query::Unsupported> {
        avian3d::parry::query::DefaultQueryDispatcher.cast_shapes(p, velocity, a, b, options)
    }
    fn cast_shapes_nonlinear(
        &self,
        m1: &NonlinearRigidMotion,
        a: &dyn avian3d::parry::shape::Shape,
        m2: &NonlinearRigidMotion,
        b: &dyn avian3d::parry::shape::Shape,
        start: f64,
        end: f64,
        stop: bool,
    ) -> Result<Option<ShapeCastHit>, avian3d::parry::query::Unsupported> {
        avian3d::parry::query::DefaultQueryDispatcher
            .cast_shapes_nonlinear(m1, a, m2, b, start, end, stop)
    }
}

fn native_nonlinear_cast(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
) -> Result<Option<ShapeCastHit>, String> {
    use avian3d::parry::query::details::{
        NonlinearShapeCastMode, cast_shapes_nonlinear_support_map_support_map,
    };
    if let (Some(support1), Some(support2)) = (s1.as_support_map(), s2.as_support_map()) {
        let queries = CheckedNativeQueries::default();
        let hit = cast_shapes_nonlinear_support_map_support_map(
            &queries,
            m1,
            support1,
            s1,
            m2,
            support2,
            s2,
            0.0,
            1.0,
            NonlinearShapeCastMode::directional_toi(s1, s2),
        );
        return match queries
            .failure
            .into_inner()
            .map_err(|_| "continuous native query error owner was poisoned".to_string())?
        {
            Some(error) => Err(error),
            None => Ok(hit),
        };
    }
    cast_shapes_nonlinear(m1, s1, m2, s2, 0.0, 1.0, false)
        .map_err(|error| format!("unsupported continuous shapes: {error:?}"))
}
/// Reuse native composite BVHs and cast kernels, retaining each part's impact.
/// Stationary composites use the other whole-arc AABB; moving composites use a
/// conservative sphere in their COM frame.
fn shape_hits(
    mode: SweepMode,
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    bounds1: &Aabb,
    normals1: Option<&dyn NormalConstraints>,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    bounds2: &Aabb,
    normals2: Option<&dyn NormalConstraints>,
    queries: &mut usize,
    tolerance: f64,
    cache: &mut InitialContactCache,
) -> Result<Vec<ShapeCastHit>, String> {
    if let Some(composite) = s1.as_composite_shape() {
        let local_bounds = if m1.linvel == DVec3::ZERO && m1.angvel == DVec3::ZERO {
            bounds2.transform_by(&m1.start.inverse())
        } else {
            // The other whole-arc AABB is enclosed by a sphere. Distance to
            // this body's linear COM path is maximized at a path endpoint;
            // any rotation preserves that distance in the local COM frame.
            let sphere = bounds2.bounding_sphere();
            let center = m1.start.translation + m1.start.rotation * m1.local_center;
            let radius = sphere.radius
                + (sphere.center - center)
                    .length()
                    .max((sphere.center - center - m1.linvel).length());
            if !radius.is_finite() {
                return Err("continuous composite bound exceeded its finite numeric range".into());
            }
            Aabb::from_half_extents(m1.local_center, DVec3::splat(radius))
        };
        let mut parts = composite
            .bvh()
            .intersect_aabb(&local_bounds)
            .collect::<Vec<_>>();
        parts.sort_unstable();
        let mut hits = Vec::new();
        for id in parts {
            let mut result = None;
            composite.map_part_at(id, &mut |pose, part, normals| {
                let part_motion = pose.map_or(*m1, |pose| m1.prepend(*pose));
                result = Some(
                    shape_hits(
                        mode,
                        &part_motion,
                        part,
                        bounds1,
                        normals,
                        m2,
                        s2,
                        bounds2,
                        normals2,
                        queries,
                        tolerance,
                        cache.parts.entry(id).or_default(),
                    )
                    .map(|hits| {
                        hits.into_iter()
                            .map(|hit| pose.map_or(hit, |pose| hit.transform1_by(pose)))
                            .collect::<Vec<_>>()
                    }),
                );
            });
            let hit = result
                .ok_or("continuous composite BVH has a stale part identity")?
                .map_err(|error| format!("continuous composite part {id}: {error}"))?;
            hits.extend(hit);
        }
        return Ok(hits);
    }
    if s2.as_composite_shape().is_some() {
        return shape_hits(
            mode, m2, s2, bounds2, normals2, m1, s1, bounds1, normals1, queries, tolerance, cache,
        )
        .map(|hits| hits.into_iter().map(ShapeCastHit::swapped).collect());
    }
    if mode == SweepMode::NonLinear && certified_native_interval(m1, s1, m2, s2, tolerance, cache)?
    {
        return Ok(Vec::new());
    }
    *queries += 1;
    let hit = {
        let _span = bevy::log::debug_span!("ccd_native_shape_cast", shape1=?s1.shape_type(), shape2=?s2.shape_type()).entered();
        if mode == SweepMode::NonLinear {
            native_nonlinear_cast(m1, s1, m2, s2)
        } else {
            cast_shapes(
                &m1.start,
                m1.linvel,
                s1,
                &m2.start,
                m2.linvel,
                s2,
                ShapeCastOptions {
                    max_time_of_impact: 1.0,
                    stop_at_penetration: false,
                    ..default()
                },
            )
            .map_err(|error| format!("unsupported continuous shapes: {error:?}"))
        }
    };
    let Some(mut hit) = hit? else {
        return Ok(Vec::new());
    };
    if hit.time_of_impact > 0.0
        && hit.time_of_impact < 1.0
        && hit.status != ShapeCastStatus::PenetratingOrWithinTargetDist
    {
        if !hit.normal1.is_finite()
            || !hit.normal2.is_finite()
            || (hit.normal1.length_squared() - 1.0).abs() > 1e-6
            || (hit.normal2.length_squared() - 1.0).abs() > 1e-6
        {
            return Err("continuous cast returned invalid native normals".into());
        }
        let relative = m1.position_at_time(hit.time_of_impact).inverse()
            * m2.position_at_time(hit.time_of_impact);
        // Consume the same native internal-edge/backface contract as contact
        // manifolds. Invalid feature normals do not describe mesh contact.
        if !(normals1, normals2).project_local_normals(
            &relative,
            &mut hit.normal1,
            &mut hit.normal2,
        ) {
            return Ok(Vec::new());
        }
    }
    Ok(vec![hit])
}
fn sweep(
    world: &World,
    contacts: &mut ContinuousContacts,
    tolerance: f64,
    diagnostics: &mut ContinuousCollisionDiagnostics,
    initial_contacts: &mut HashMap<usize, InitialContactCache>,
) -> Result<Vec<Plane>, String> {
    let begin = bevy::platform::time::Instant::now();
    let result = sweep_geometry(world, contacts, tolerance, diagnostics, initial_contacts);
    diagnostics.sweep_time += begin.elapsed();
    result
}
fn sweep_geometry(
    world: &World,
    contacts: &mut ContinuousContacts,
    tolerance: f64,
    diagnostics: &mut ContinuousCollisionDiagnostics,
    initial_contacts: &mut HashMap<usize, InitialContactCache>,
) -> Result<Vec<Plane>, String> {
    let _span = bevy::log::debug_span!(
        "ccd_sweep_geometry",
        eligible_pairs = contacts.pairs.len(),
        shapes = contacts.shapes.len()
    )
    .entered();
    // The native contact graph is prepared once per physics step. Keep newly
    // admitted impact witnesses through its remaining substeps so zero-time
    // touching cannot lose the contact before the next native preparation.
    let mut planes = Vec::new();
    let dummy = SolverBody::default();
    // This World is immutable for one sweep. Reuse each leaf's exact motion
    // and bound across candidate pairs, then rebuild after any correction.
    let dt = world
        .get_resource::<Time<Substeps>>()
        .ok_or("continuous collision has no substep clock")?
        .delta_secs_f64();
    if !dt.is_finite() || dt <= 0.0 {
        return Err("invalid continuous collision substep clock".into());
    }
    for plane in contacts.prepared.iter().chain(&contacts.impacts) {
        let s1 = contacts.starts.get(&plane.body1).unwrap_or(&dummy);
        let s2 = contacts.starts.get(&plane.body2).unwrap_or(&dummy);
        let e1 = world.get::<SolverBody>(plane.body1).unwrap_or(&dummy);
        let e2 = world.get::<SolverBody>(plane.body2).unwrap_or(&dummy);
        if minimum_gap(plane, s1, e1, s2, e2, tolerance)?.1 < -tolerance {
            planes.push(plane.clone());
        }
    }
    let trajectories = {
        let _span = bevy::log::debug_span!("ccd_leaf_trajectories").entered();
        let mut trajectories = Vec::with_capacity(contacts.shapes.len());
        let mut bodies = HashMap::new();
        for shape in &contacts.shapes {
            let body = match bodies.entry(shape.body) {
                bevy::platform::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                bevy::platform::collections::hash_map::Entry::Vacant(entry) => {
                    let start = contacts.starts.get(&shape.body).unwrap_or(&dummy);
                    let end = world.get::<SolverBody>(shape.body).unwrap_or(&dummy);
                    // Endpoint quaternions do not retain complete revolutions. Refuse an
                    // ambiguous integration increment before the logarithm can hide it.
                    if end.angular_velocity.length() * dt >= std::f64::consts::PI {
                        return Err("continuous collision requires an angular integration increment below pi radians per substep".into());
                    }
                    entry.insert(BodyMotion::new(start, end)?)
                }
            };
            let motion = motion(shape, body);
            let bounds = swept_bounds(shape, &motion);
            if !bounds.mins.is_finite()
                || !bounds.maxs.is_finite()
                || !bounds.mins.cmple(bounds.maxs).all()
            {
                return Err("continuous swept bounds exceeded their finite numeric range".into());
            }
            trajectories.push((motion, bounds));
        }
        trajectories
    };
    let candidates = swept_pair_candidates(contacts, &trajectories)?;
    let _span =
        bevy::log::debug_span!("ccd_candidate_geometry", candidates = candidates.len()).entered();
    let mut jobs = Vec::with_capacity(candidates.len());
    for pair_index in candidates {
        let pair = &contacts.pairs[pair_index];
        let (m1, _) = trajectories[pair.shapes[0]];
        let (m2, _) = trajectories[pair.shapes[1]];
        // Unchanged poses cannot admit a new continuous crossing. Prepared
        // native contact inequalities above still own their existing contact.
        if m1.linvel == DVec3::ZERO
            && m1.angvel == DVec3::ZERO
            && m2.linvel == DVec3::ZERO
            && m2.angvel == DVec3::ZERO
        {
            continue;
        }
        let linear = (m2.linvel - m1.linvel).length() / dt;
        let angular = (m2.angvel - m1.angvel).length() / dt;
        if !pair
            .ccd
            .into_iter()
            .flatten()
            .any(|ccd| linear >= ccd.linear_threshold || angular >= ccd.angular_threshold)
        {
            continue;
        }
        jobs.push(SweepJob {
            pair_index,
            cache: initial_contacts.remove(&pair_index).unwrap_or_default(),
        });
    }
    let results = query_sweep_jobs(
        &contacts.shapes,
        &contacts.pairs,
        &trajectories,
        &mut jobs,
        tolerance,
        world
            .resource::<ContinuousCollisionSettings>()
            .minimum_query_batch_size,
        ComputeTaskPool::try_get().ok_or("continuous collision compute pool is not installed")?,
    );
    for (job, result) in jobs.into_iter().zip(results) {
        initial_contacts.insert(job.pair_index, job.cache);
        diagnostics.sweep_queries += result.queries;
        let hits = result.hits?;
        let pair = &contacts.pairs[job.pair_index];
        let a = &contacts.shapes[pair.shapes[0]];
        let b = &contacts.shapes[pair.shapes[1]];
        let (m1, _) = trajectories[pair.shapes[0]];
        for hit in hits {
            // Initial touching/overlap belongs to the native discrete solver. Its
            // cast witnesses are not impact geometry and must never become planes.
            if hit.time_of_impact <= 0.0 || hit.time_of_impact >= 1.0 {
                continue;
            }
            if hit.status == ShapeCastStatus::PenetratingOrWithinTargetDist {
                return Err("positive-time sweep returned initial-penetration geometry".into());
            }
            let normal = m1.position_at_time(hit.time_of_impact).rotation * hit.normal1;
            if !normal.is_finite()
                || (normal.length_squared() - 1.0).abs() > 1e-6
                || !hit.witness1.is_finite()
                || !hit.witness2.is_finite()
            {
                return Err(format!(
                    "invalid continuous contact witnesses for {:?} / {:?}",
                    a.entity, b.entity
                ));
            }
            // Keep the actual material witnesses. A constraint is evaluated over
            // their complete trajectories, including an arc that exits before t=1.
            let plane = Plane {
                collider1: a.entity,
                collider2: b.entity,
                part1: a.part_index,
                part2: b.part_index,
                body1: a.body,
                body2: b.body,
                center1: a.center,
                center2: b.center,
                point1: (a.position - a.center) + a.rotation * hit.witness1,
                point2: (b.position - b.center) + b.rotation * hit.witness2,
                normal,
                offset: 0.0,
                native_point: None,
            };
            let s1 = contacts.starts.get(&a.body).unwrap_or(&dummy);
            let s2 = contacts.starts.get(&b.body).unwrap_or(&dummy);
            let e1 = world.get::<SolverBody>(a.body).unwrap_or(&dummy);
            let e2 = world.get::<SolverBody>(b.body).unwrap_or(&dummy);
            let (_, gap) = minimum_gap(&plane, s1, e1, s2, e2, tolerance)?;
            if gap < -tolerance {
                planes.push(plane);
            }
        }
    }
    Ok(planes)
}

/// Admit only overlapping whole-arc bounds through Parry's native BVH. Stable
/// prepared pair indices preserve collision-hook decisions and sweep order.
/// Every corrected trajectory replaces leaf bounds and refits native ancestors.
fn swept_pair_candidates(
    contacts: &mut ContinuousContacts,
    trajectories: &[(NonlinearRigidMotion, Aabb)],
) -> Result<Vec<usize>, String> {
    let _span = bevy::log::debug_span!("ccd_swept_bvh").entered();
    use avian3d::parry::partitioning::{Bvh, BvhBuildStrategy};
    u32::try_from(trajectories.len())
        .map_err(|_| "continuous shape count exceeds native BVH identity range")?;
    let bounds = trajectories
        .iter()
        .map(|(_, bounds)| *bounds)
        .collect::<Vec<_>>();
    if contacts.swept_bvh.leaf_count() == 0 {
        contacts.swept_bvh = Bvh::from_leaves(BvhBuildStrategy::Binned, &bounds);
    } else {
        // Leaf identities remain fixed for the prepared physics step. Replace
        // every bound exactly, then refit ancestors without an admission margin
        // or a topology rebuild between closure sweeps.
        for (index, bounds) in bounds.iter().enumerate() {
            contacts
                .swept_bvh
                .insert_or_update_partially(*bounds, index as u32, 0.0);
        }
        contacts.swept_bvh.refit_without_opt();
    }
    let tree = &contacts.swept_bvh;
    let mut candidates = std::collections::BTreeSet::new();
    for &index in &contacts.active_shapes {
        for other in tree.intersect_aabb(&bounds[index]) {
            let other = other as usize;
            if index == other {
                continue;
            }
            if let Some(&pair) = contacts
                .pair_indices
                .get(&(index.min(other), index.max(other)))
            {
                candidates.insert(pair);
            }
        }
    }
    Ok(candidates.into_iter().collect())
}
/// Left SO(3) Jacobian and inverse, with their analytic limits at zero.
fn rotation_jacobian(v: DVec3, inverse: bool) -> DMat3 {
    let x = DMat3::from_cols(v.cross(DVec3::X), v.cross(DVec3::Y), v.cross(DVec3::Z));
    let theta = v.length();
    if inverse {
        let c = if theta < 1e-4 {
            1.0 / 12.0 + theta * theta / 720.0
        } else {
            (1.0 - 0.5 * theta / (0.5 * theta).tan()) / (theta * theta)
        };
        DMat3::IDENTITY - 0.5 * x + c * x * x
    } else {
        let (a, b) = if theta < 1e-4 {
            (
                0.5 - theta * theta / 24.0,
                1.0 / 6.0 - theta * theta / 120.0,
            )
        } else {
            (
                (1.0 - theta.cos()) / (theta * theta),
                (theta - theta.sin()) / (theta * theta * theta),
            )
        };
        DMat3::IDENTITY + a * x + b * x * x
    }
}
struct WitnessMotion {
    start: DVec3,
    end: DVec3,
    omega: DVec3,
    point: DVec3,
}
impl WitnessMotion {
    fn new(start: &SolverBody, end: &SolverBody, point: DVec3) -> Result<Self, String> {
        Ok(Self {
            start: start.delta_position,
            end: end.delta_position,
            omega: rotation_displacement(start.delta_rotation.0, end.delta_rotation.0)?,
            point: start.delta_rotation.0 * point,
        })
    }
    fn rotated_point(&self, t: f64) -> DVec3 {
        DQuat::from_scaled_axis(self.omega * t) * self.point
    }
    fn position_and_velocity(&self, t: f64) -> (DVec3, DVec3) {
        let r = self.rotated_point(t);
        (
            self.start.lerp(self.end, t) + r,
            self.end - self.start + self.omega.cross(r),
        )
    }
    fn gradient(&self, t: f64, normal: DVec3) -> DVec3 {
        let k = t * rotation_jacobian(self.omega * t, false) * rotation_jacobian(self.omega, true);
        k.transpose() * self.rotated_point(t).cross(normal)
    }
}
/// Bound and minimize the separation of material witnesses throughout the
/// substep. Taylor's remainder bounds each interval; no temporal samples are
/// treated as proof that the intervening arc is clear.
fn minimum_gap(
    p: &Plane,
    s1: &SolverBody,
    e1: &SolverBody,
    s2: &SolverBody,
    e2: &SolverBody,
    tolerance: f64,
) -> Result<(f64, f64), String> {
    let _span =
        bevy::log::debug_span!(target: "lunco_physics::ccd::minimum", "ccd_minimum_gap").entered();
    let motion1 = WitnessMotion::new(s1, e1, p.point1)?;
    let motion2 = WitnessMotion::new(s2, e2, p.point2)?;
    if ![
        p.center1,
        p.center2,
        p.point1,
        p.point2,
        p.normal,
        motion1.start,
        motion1.end,
        motion1.omega,
        motion1.point,
        motion2.start,
        motion2.end,
        motion2.omega,
        motion2.point,
    ]
    .into_iter()
    .all(DVec3::is_finite)
        || !p.offset.is_finite()
        || (p.normal.length_squared() - 1.0).abs() > 1e-6
    {
        return Err("continuous contact has invalid native witness geometry or motion".into());
    }
    let evaluate = |t: f64| {
        let (r1, d1) = motion1.position_and_velocity(t);
        let (r2, d2) = motion2.position_and_velocity(t);
        (
            ((p.center2 - p.center1) + (r2 - r1)).dot(p.normal) + p.offset,
            (d2 - d1).dot(p.normal),
        )
    };
    let (initial, initial_derivative) = evaluate(0.0);
    // Existing penetration is owned by the discrete contact solver. CCD must
    // not deepen it or introduce a new crossing during this substep.
    let baseline = initial.min(0.0);
    // Rotation preserves |omega cross r|. Project the second-derivative
    // bound onto the contact normal, rather than bounding all three axes.
    let acceleration = motion1.omega.cross(motion1.point).length()
        * p.normal.cross(motion1.omega).length()
        + motion2.omega.cross(motion2.point).length() * p.normal.cross(motion2.omega).length();
    let mut best = (0.0, initial);
    let (end, end_derivative) = evaluate(1.0);
    if !initial.is_finite() || !end.is_finite() || !acceleration.is_finite() {
        return Err("continuous witness bound exceeded its finite numeric range".into());
    }
    if end < best.1 {
        best = (1.0, end);
    }
    // The complete material trajectory stays above its endpoint chord minus
    // this second-derivative remainder. A clear bound needs no root search.
    let chord_lower = initial.min(end) - acceleration / 8.0;
    if chord_lower >= baseline - tolerance {
        return Ok((best.0, chord_lower - baseline));
    }
    // A derivative that cannot reach zero certifies the endpoint minimum.
    // This is an analytic whole-interval bound, including zero rotation.
    if initial_derivative >= acceleration || end_derivative >= acceleration {
        return Ok((0.0, initial - baseline));
    }
    if initial_derivative <= -acceleration || end_derivative <= -acceleration {
        return Ok((1.0, end - baseline));
    }
    let mut intervals = vec![(0.0, 1.0)];
    let mut count = 0;
    let mut lower = f64::INFINITY;
    while let Some((lo, hi)) = intervals.pop() {
        count += 1;
        if count > 4096 {
            return Err("continuous contact arc bound did not converge".into());
        }
        let t = (lo + hi) * 0.5;
        let half = (hi - lo) * 0.5;
        let (g, d) = evaluate(t);
        if g < best.1 {
            best = (t, g);
        }
        let bound = g - d.abs() * half - 0.5 * acceleration * half * half;
        if bound < best.1 - tolerance * 0.25 {
            intervals.push((t, hi));
            intervals.push((lo, t));
        } else {
            lower = lower.min(bound);
        }
    }
    let (t, g) = best;
    Ok((t, lower.min(g) - baseline))
}
fn project(
    world: &mut World,
    plane: &Plane,
    contacts: &mut ContinuousContacts,
    tolerance: f64,
) -> Result<f64, String> {
    let dummy = SolverBody::default();
    let b1 = world
        .get::<SolverBody>(plane.body1)
        .unwrap_or(&dummy)
        .clone();
    let b2 = world
        .get::<SolverBody>(plane.body2)
        .unwrap_or(&dummy)
        .clone();
    let mut i1 = world
        .get::<SolverBodyInertia>(plane.body1)
        .cloned()
        .unwrap_or(SolverBodyInertia::DUMMY);
    let mut i2 = world
        .get::<SolverBodyInertia>(plane.body2)
        .cloned()
        .unwrap_or(SolverBodyInertia::DUMMY);
    match i1.dominance().cmp(&i2.dominance()) {
        std::cmp::Ordering::Greater => i1 = SolverBodyInertia::DUMMY,
        std::cmp::Ordering::Less => i2 = SolverBodyInertia::DUMMY,
        _ => {}
    }
    let s1 = contacts.starts.get(&plane.body1).unwrap_or(&dummy);
    let s2 = contacts.starts.get(&plane.body2).unwrap_or(&dummy);
    let (t, gap) = minimum_gap(plane, s1, &b1, s2, &b2, tolerance)?;
    if gap >= -tolerance {
        return Ok(0.0);
    }
    let a1 = WitnessMotion::new(s1, &b1, plane.point1)?.gradient(t, plane.normal);
    let a2 = WitnessMotion::new(s2, &b2, plane.point2)?.gradient(t, plane.normal);
    let n = plane.normal * t;
    let w = n.dot((i1.effective_inv_mass() + i2.effective_inv_mass()) * n)
        + a1.dot(i1.effective_inv_angular_inertia() * a1)
        + a2.dot(i2.effective_inv_angular_inertia() * a2);
    if !w.is_finite() || w <= 0.0 {
        return Err(
            "continuous collision has no finite movable constraint degree of freedom".into(),
        );
    }
    let lambda = -gap / w;
    let mut b1 = b1;
    let mut b2 = b2;
    b1.delta_position -= i1.effective_inv_mass() * n * lambda;
    b2.delta_position += i2.effective_inv_mass() * n * lambda;
    b1.delta_rotation.0 =
        DQuat::from_scaled_axis(-(i1.effective_inv_angular_inertia() * a1) * lambda)
            * b1.delta_rotation.0;
    b2.delta_rotation.0 =
        DQuat::from_scaled_axis((i2.effective_inv_angular_inertia() * a2) * lambda)
            * b2.delta_rotation.0;
    if let Some(mut body) = world.get_mut::<SolverBody>(plane.body1) {
        *body = b1;
    }
    if let Some(mut body) = world.get_mut::<SolverBody>(plane.body2) {
        *body = b2;
    }
    let dt = world.resource::<Time<Substeps>>().delta_secs_f64();
    let impulse = lambda * t / dt;
    if let Some((color, index, point)) = plane.native_point {
        let mut graph = world.resource_mut::<ConstraintGraph>();
        let constraint = graph
            .colors
            .get_mut(color)
            .and_then(|color| color.contact_constraints.get_mut(index))
            .ok_or("prepared continuous contact identity became stale")?;
        if !((constraint.body1 == plane.body1
            && constraint.body2 == plane.body2
            && constraint.normal == plane.normal)
            || (constraint.body1 == plane.body2
                && constraint.body2 == plane.body1
                && constraint.normal == -plane.normal))
        {
            return Err(
                "prepared continuous contact no longer owns its native body pair and normal".into(),
            );
        }
        let point = constraint
            .points
            .get_mut(point)
            .ok_or("prepared continuous contact point became stale")?;
        // Position projection already applies this impulse through native
        // velocity projection. Publish it once to the same normal accumulator
        // used by friction, restitution, force facts and warm starting.
        point.normal_part.impulse += impulse;
        point.normal_part.total_impulse += impulse;
    } else {
        if let Some(applied) = contacts.impulses.iter_mut().find(|p| {
            p.collider1 == plane.collider1
                && p.collider2 == plane.collider2
                && p.part1 == plane.part1
                && p.part2 == plane.part2
        }) {
            applied.impulse += impulse;
        } else {
            contacts.impulses.push(ContinuousContactImpulse {
                collider1: plane.collider1,
                collider2: plane.collider2,
                part1: plane.part1,
                part2: plane.part2,
                impulse,
            });
        }
    }
    Ok(-gap)
}

fn affected_joint_bodies(
    world: &World,
    planes: &[Plane],
    affected: &mut std::collections::HashSet<Entity>,
) -> Result<(), String> {
    use avian3d::dynamics::joints::EntityConstraint;
    for plane in planes {
        affected.extend([plane.body1, plane.body2]);
    }
    let order = world.resource::<JointSolverOrder>();
    let mut edges = Vec::new();
    macro_rules! edges {
        ($kind:ty,$ids:expr) => {
            for entity in $ids {
                let joint = world.get::<$kind>(*entity).ok_or_else(|| {
                    format!("continuous closure has a stale native joint {entity:?}")
                })?;
                edges.push(joint.entities());
            }
        };
    }
    edges!(FixedJoint, &order.fixed);
    edges!(RevoluteJoint, &order.revolute);
    edges!(SphericalJoint, &order.spherical);
    edges!(PrismaticJoint, &order.prismatic);
    edges!(DistanceJoint, &order.distance);
    loop {
        let before = affected.len();
        for [a, b] in &edges {
            if affected.contains(a) || affected.contains(b) {
                affected.extend([*a, *b]);
            }
        }
        if affected.len() == before {
            break;
        }
    }
    Ok(())
}

fn violated_planes(
    world: &World,
    contacts: &ContinuousContacts,
    admitted: &[Plane],
    tolerance: f64,
) -> Result<Vec<Plane>, String> {
    let dummy = SolverBody::default();
    let mut violated = Vec::new();
    for plane in admitted {
        let start1 = contacts.starts.get(&plane.body1).unwrap_or(&dummy);
        let start2 = contacts.starts.get(&plane.body2).unwrap_or(&dummy);
        let end1 = world.get::<SolverBody>(plane.body1).unwrap_or(&dummy);
        let end2 = world.get::<SolverBody>(plane.body2).unwrap_or(&dummy);
        if minimum_gap(plane, start1, end1, start2, end2, tolerance)?.1 < -tolerance {
            violated.push(plane.clone());
        }
    }
    Ok(violated)
}

fn same_plane(a: &Plane, b: &Plane) -> bool {
    a.collider1 == b.collider1
        && a.collider2 == b.collider2
        && a.part1 == b.part1
        && a.part2 == b.part2
        && a.body1 == b.body1
        && a.body2 == b.body2
        && a.point1 == b.point1
        && a.point2 == b.point2
        && a.normal == b.normal
        && a.offset == b.offset
        && a.native_point == b.native_point
}

fn solve_contacts(world: &mut World) {
    if world.resource::<lunco_core::RuntimeFaults>().active() {
        return;
    }
    let settings = world.resource::<ContinuousCollisionSettings>().clone();
    let begin = bevy::platform::time::Instant::now();
    let mut diagnostics = ContinuousCollisionDiagnostics::default();
    let mut contacts = world
        .remove_resource::<ContinuousContacts>()
        .expect("continuous collision owner initialized");
    let mut initial_contacts = std::mem::take(&mut contacts.initial_contacts);
    let outcome = (|| -> Result<(), String> {
        if settings.max_iterations == 0
            || !settings.position_tolerance_m.is_finite()
            || settings.position_tolerance_m <= 0.0
            || !settings.angular_tolerance_rad.is_finite()
            || settings.angular_tolerance_rad <= 0.0
        {
            return Err("invalid continuous collision convergence settings".into());
        }
        // Starts and collider geometry remain immutable until this substep ends.
        let mut planes = sweep(
            world,
            &mut contacts,
            settings.position_tolerance_m,
            &mut diagnostics,
            &mut initial_contacts,
        )?;
        if planes.is_empty() {
            return Ok(());
        }
        let mut admitted = planes.clone();
        let mut affected = std::collections::HashSet::new();
        affected_joint_bodies(world, &planes, &mut affected)?;
        world.resource_mut::<PhysicsJointPositionScope>().0 = Some(affected.clone());
        let mut joint_position_change = 0.0f64;
        let mut joint_rotation_change = 0.0f64;
        for _ in 0..settings.max_iterations {
            diagnostics.position_iterations += 1;
            for plane in &planes {
                project(world, plane, &mut contacts, settings.position_tolerance_m)?;
                // Admit each material plane once; witness coordinates are in the
                // step's fixed reference frame, shared by every substep.
                if plane.native_point.is_none()
                    && !contacts.impacts.iter().any(|p| same_plane(p, plane))
                {
                    contacts.impacts.push(plane.clone());
                }
            }
            let before = affected
                .iter()
                .filter_map(|e| world.get::<SolverBody>(*e).map(|b| (*e, b.clone())))
                .collect::<Vec<_>>();
            let joint_begin = bevy::platform::time::Instant::now();
            world.run_schedule(PhysicsJointPositionSchedule);
            diagnostics.joint_time += joint_begin.elapsed();
            if world.resource::<lunco_core::RuntimeFaults>().active() {
                return Err("native hard-joint closure rejected the continuous solve".into());
            }
            joint_position_change = 0.0;
            joint_rotation_change = 0.0;
            for (entity, before) in &before {
                let after = world
                    .get::<SolverBody>(*entity)
                    .ok_or("joint solve removed a participating solver body")?;
                joint_position_change = joint_position_change
                    .max((after.delta_position - before.delta_position).length());
                joint_rotation_change = joint_rotation_change.max(
                    rotation_displacement(before.delta_rotation.0, after.delta_rotation.0)?
                        .length(),
                );
            }
            // Material witnesses remain real points of the admitted shapes.
            // Close their exact trajectories and native joints before querying
            // the full geometry again; only a revalidated sweep may commit.
            planes = violated_planes(world, &contacts, &admitted, settings.position_tolerance_m)?;
            if planes.is_empty()
                && joint_position_change <= settings.position_tolerance_m
                && joint_rotation_change <= settings.angular_tolerance_rad
            {
                planes = sweep(
                    world,
                    &mut contacts,
                    settings.position_tolerance_m,
                    &mut diagnostics,
                    &mut initial_contacts,
                )?;
                if planes.is_empty() {
                    return Ok(());
                }
                for plane in &planes {
                    if !admitted.iter().any(|p| same_plane(p, plane)) {
                        admitted.push(plane.clone());
                    }
                }
                affected_joint_bodies(world, &planes, &mut affected)?;
                world.resource_mut::<PhysicsJointPositionScope>().0 = Some(affected.clone());
            }
        }
        let mut remaining = Vec::new();
        let dummy = SolverBody::default();
        for plane in &planes {
            let s1 = contacts.starts.get(&plane.body1).unwrap_or(&dummy);
            let s2 = contacts.starts.get(&plane.body2).unwrap_or(&dummy);
            let e1 = world.get::<SolverBody>(plane.body1).unwrap_or(&dummy);
            let e2 = world.get::<SolverBody>(plane.body2).unwrap_or(&dummy);
            let (time, gap) = minimum_gap(plane, s1, e1, s2, e2, settings.position_tolerance_m)?;
            let first_motion = WitnessMotion::new(s1, e1, plane.point1)?;
            let second_motion = WitnessMotion::new(s2, e2, plane.point2)?;
            let separation = |time| {
                let a = first_motion.position_and_velocity(time).0;
                let b = second_motion.position_and_velocity(time).0;
                ((plane.center2 - plane.center1) + (b - a)).dot(plane.normal) + plane.offset
            };
            remaining.push(format!(
                "{:?}/{:?}: native={}, t={time:e}, gap={gap:e}, start={:e}, end={:e}, normal={:?}",
                world.get::<crate::PhysicsOrderKey>(plane.body1),
                world.get::<crate::PhysicsOrderKey>(plane.body2),
                plane.native_point.is_some(),
                separation(0.0),
                separation(1.0),
                plane.normal
            ));
        }
        Err(format!(
            "continuous contact and joint constraints did not converge; pose writeback refused (remaining contacts={}, joint position change={joint_position_change:e} m, rotation change={joint_rotation_change:e} rad): {}",
            planes.len(),
            remaining.join("; ")
        ))
    })();
    contacts.initial_contacts = initial_contacts;
    world.resource_mut::<PhysicsJointPositionScope>().0 = None;
    world.insert_resource(contacts);
    let mut total = world.resource_mut::<ContinuousCollisionDiagnostics>();
    total.substeps += 1;
    total.sweep_queries += diagnostics.sweep_queries;
    total.position_iterations += diagnostics.position_iterations;
    total.sweep_time += diagnostics.sweep_time;
    total.joint_time += diagnostics.joint_time;
    total.solve_time += begin.elapsed();
    if let Err(detail) = outcome {
        world
            .resource_mut::<PhysicsHolds>()
            .set(PhysicsHolds::SAFETY_FAILURE, true);
        world.resource_mut::<lunco_core::RuntimeFaults>().raise(
            "physics-continuous-contact-invalid",
            None,
            "continuous contact solve",
            detail,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::math::SymmetricMatrix;

    #[test]
    fn continuous_initial_contact_uses_native_relative_pose_at_large_translation() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = Cylinder::new(0.1, 0.2);
        let floor = Pose::from_parts(
            DVec3::splat(1e11),
            DQuat::from_euler(bevy::math::EulerRot::XYZ, 0.37, -0.71, 0.23),
        );
        let body = Pose::from_parts(
            floor.translation + floor.rotation * DVec3::new(-0.5, 0.10002, -0.5),
            floor.rotation,
        );
        let relative = floor.inv_mul(&body);
        let fixed = NonlinearRigidMotion::constant_position(floor);
        let moving = NonlinearRigidMotion::constant_position(body);
        let local_fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let local_moving = NonlinearRigidMotion::constant_position(relative);
        let axis = floor.rotation * DVec3::Y;

        let world =
            initial_native_contact(&fixed, &triangle, &moving, &cylinder, 2.0, axis).unwrap();
        let local = initial_native_contact(
            &local_fixed,
            &triangle,
            &local_moving,
            &cylinder,
            2.0,
            DVec3::Y,
        )
        .unwrap();
        assert_eq!(
            world.dist, local.dist,
            "world origin changed native initial clearance"
        );
    }

    #[test]
    fn continuous_material_gap_retains_crossing_at_large_translation() {
        let base = 1e11;
        let plane = Plane {
            collider1: Entity::PLACEHOLDER,
            collider2: Entity::PLACEHOLDER,
            part1: 0,
            part2: 0,
            body1: Entity::PLACEHOLDER,
            body2: Entity::PLACEHOLDER,
            center1: DVec3::Y * base,
            center2: DVec3::Y * (base + 0.10002),
            point1: DVec3::ZERO,
            point2: -DVec3::Y * 0.10001,
            normal: DVec3::Y,
            offset: 0.0,
            native_point: None,
        };
        let start = SolverBody::default();
        let end = SolverBody {
            delta_position: -DVec3::Y * 0.0000152587890625,
            ..default()
        };
        let (time, gap) = minimum_gap(&plane, &start, &start, &start, &end, 1e-6).unwrap();
        assert_eq!(time, 1.0);
        assert!(gap < -1e-6, "material crossing rounded away: {gap:e}");
        let mut local = plane.clone();
        local.center2 -= local.center1;
        local.center1 = DVec3::ZERO;
        assert_eq!(
            minimum_gap(&local, &start, &start, &start, &end, 1e-6).unwrap(),
            (time, gap)
        );
    }

    #[test]
    fn continuous_scoped_queries_preserve_order_geometry_and_errors() {
        let bits = |value: DVec3| value.to_array().map(f64::to_bits);
        let contact_bits = |contact: avian3d::parry::query::Contact| {
            (
                bits(contact.point1),
                bits(contact.point2),
                bits(contact.normal1),
                bits(contact.normal2),
                contact.dist.to_bits(),
            )
        };
        let shape = |collider| Shape {
            entity: Entity::PLACEHOLDER,
            part_index: 0,
            body: Entity::PLACEHOLDER,
            collider,
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            center: DVec3::ZERO,
        };
        let mut shapes = vec![shape(Collider::from(
            avian3d::parry::shape::SharedShape::triangle(
                DVec3::new(-10.0, 0.0, -10.0),
                DVec3::new(-10.0, 0.0, 10.0),
                DVec3::new(10.0, 0.0, -10.0),
            ),
        ))];
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let mut trajectories = vec![(fixed, swept_bounds(&shapes[0], &fixed))];
        for (height, linear) in [
            (0.3, -DVec3::Y * 0.5),
            (0.099, DVec3::X * 0.5),
            (0.3, DVec3::Y),
            (0.1, -DVec3::Y * 0.1),
            (0.3, DVec3::splat(f64::NAN)),
        ] {
            shapes.push(shape(Collider::cylinder(0.2, 0.2)));
            let moving = NonlinearRigidMotion::new(
                Pose::from_translation(DVec3::new(-1.0, height, -1.0)),
                DVec3::ZERO,
                linear,
                DVec3::ZERO,
            );
            // Invalid motion is intentionally sent to the native query boundary;
            // production admission rejects its swept bounds before dispatch.
            let bounds = if linear.is_finite() {
                swept_bounds(shapes.last().unwrap(), &moving)
            } else {
                Aabb::from_half_extents(DVec3::ZERO, DVec3::ONE)
            };
            trajectories.push((moving, bounds));
        }
        let pairs = (1..shapes.len())
            .map(|index| SweepPair {
                shapes: [0, index],
                ccd: [None, None],
                mode: SweepMode::NonLinear,
            })
            .collect::<Vec<_>>();
        let jobs = || {
            [4, 0, 3, 1, 2]
                .into_iter()
                .map(|pair_index| SweepJob {
                    pair_index,
                    cache: InitialContactCache::default(),
                })
                .collect::<Vec<_>>()
        };
        let mut serial_jobs = jobs();
        let mut parallel_jobs = jobs();
        let serial_pool = bevy::tasks::TaskPoolBuilder::new().num_threads(4).build();
        let parallel_pool = bevy::tasks::TaskPoolBuilder::new().num_threads(4).build();

        for _ in 0..2 {
            let serial = query_sweep_jobs(
                &shapes,
                &pairs,
                &trajectories,
                &mut serial_jobs,
                1e-6,
                64,
                &serial_pool,
            );
            let parallel = query_sweep_jobs(
                &shapes,
                &pairs,
                &trajectories,
                &mut parallel_jobs,
                1e-6,
                0,
                &parallel_pool,
            );

            assert_eq!(serial.len(), 5);
            assert_eq!(parallel.len(), serial.len());
            assert!(serial[0].hits.is_err());
            assert!(
                serial[1]
                    .hits
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|hit| hit.time_of_impact > 0.0)
            );
            for (serial, parallel) in serial.iter().zip(&parallel) {
                assert_eq!(serial.queries, parallel.queries);
                match (&serial.hits, &parallel.hits) {
                    (Ok(serial), Ok(parallel)) => {
                        assert_eq!(serial.len(), parallel.len());
                        for (serial, parallel) in serial.iter().zip(parallel) {
                            assert_eq!(
                                serial.time_of_impact.to_bits(),
                                parallel.time_of_impact.to_bits()
                            );
                            assert_eq!(bits(serial.witness1), bits(parallel.witness1));
                            assert_eq!(bits(serial.witness2), bits(parallel.witness2));
                            assert_eq!(bits(serial.normal1), bits(parallel.normal1));
                            assert_eq!(bits(serial.normal2), bits(parallel.normal2));
                            assert_eq!(serial.status, parallel.status);
                        }
                    }
                    (Err(serial), Err(parallel)) => assert_eq!(serial, parallel),
                    _ => panic!("scoped native query changed its result"),
                }
            }
            for (serial, parallel) in serial_jobs.iter().zip(&parallel_jobs) {
                assert_eq!(serial.pair_index, parallel.pair_index);
                assert_eq!(
                    serial.cache.contact.map(contact_bits),
                    parallel.cache.contact.map(contact_bits)
                );
                assert_eq!(serial.cache.depth_axis, parallel.cache.depth_axis);
                assert_eq!(
                    serial.cache.candidate_axis.map(bits),
                    parallel.cache.candidate_axis.map(bits)
                );
            }
        }
    }

    #[test]
    fn continuous_native_motion_retains_exact_initial_and_linear_poses() {
        let start = Pose::new(
            DVec3::new(0.0123, -0.0234, 0.0345),
            DVec3::new(0.2, -0.3, 0.4),
        );
        let center = DVec3::new(13.0, -17.0, 29.0);
        let velocity = DVec3::new(0.012, -0.017, 0.029);
        let angular = DVec3::new(0.02, -0.03, 0.04);
        let motion = NonlinearRigidMotion::new(start, center, velocity, angular);
        assert_eq!(
            motion.position_at_time(0.0),
            start,
            "the native motion's initial pose must be the exact supplied pose"
        );
        let linear = NonlinearRigidMotion::new(start, center, velocity, DVec3::ZERO);
        for time in [0.0, 0.25, 0.5, 1.0] {
            assert_eq!(
                linear.position_at_time(time),
                Pose::from_parts(start.translation + velocity * time, start.rotation)
            );
            let pose = motion.position_at_time(time);
            let pivot = start * center + velocity * time;
            assert!((pose * center - pivot).length() < 1e-12);
            let point = DVec3::new(-2.0, 3.0, 5.0);
            let expected = pivot
                + DQuat::from_scaled_axis(angular * time) * (start.rotation * (point - center));
            assert!((pose * point - expected).length() < 1e-12);
        }
    }

    #[test]
    fn continuous_support_gap_retains_small_clearance_at_large_translation() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = Cylinder::new(0.10001, 0.2);
        let fixed =
            NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::Y * 1e11));
        let moving = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.5, 1e11 + 0.10002, -0.5)),
            DVec3::ZERO,
            -DVec3::Y * 0.0000152587890625,
            DVec3::ZERO,
        );
        let end = moving.position_at_time(1.0);
        let actual_gap = fixed.start.inv_mul(&end).translation.y - cylinder.half_height;
        assert!(actual_gap < -1e-6, "{actual_gap:e}");
        assert!(
            avian3d::parry::query::intersection_test(&fixed.start, &triangle, &end, &cylinder)
                .unwrap()
        );
        let interval = SupportInterval::new(&fixed, &triangle, &moving, &cylinder).unwrap();
        let (_, lower) = interval.gap_bound(DVec3::Y).unwrap();
        assert!(
            lower < -1e-6,
            "native relative gap {actual_gap:e}, bound {lower:e}"
        );
        assert!(
            !certified_native_interval(
                &fixed,
                &triangle,
                &moving,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap()
        );
        let reverse = SupportInterval::new(&moving, &cylinder, &fixed, &triangle).unwrap();
        assert!(reverse.gap_bound(-DVec3::Y).unwrap().1 < -1e-6);
        assert!(
            !certified_native_interval(
                &moving,
                &cylinder,
                &fixed,
                &triangle,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap()
        );
    }

    #[test]
    fn continuous_advisory_axis_revalidates_changed_substep_geometry() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = Cylinder::new(0.1, 0.2);
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let clear = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.5, 0.2, -0.5)),
            DVec3::ZERO,
            DVec3::Y * 0.001,
            DVec3::ZERO,
        );
        let mut cache = InitialContactCache::default();

        assert!(
            certified_native_interval(&fixed, &triangle, &clear, &cylinder, 1e-6, &mut cache)
                .unwrap()
        );
        let axis = cache.candidate_axis.unwrap();
        let overlap = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.5, 0.05, -0.5)),
            DVec3::ZERO,
            -DVec3::Y * 0.001,
            DVec3::ZERO,
        );
        let mut part = InitialContactCache::default();
        assert!(
            certified_native_interval(&fixed, &triangle, &overlap, &cylinder, 1e-6, &mut part)
                .unwrap()
        );
        part.depth_axis = Some(Some(DepthAxis {
            axis,
            lower_depth: 0.05,
        }));
        part.contact = Some(
            initial_native_contact(&fixed, &triangle, &overlap, &cylinder, 10.0, DVec3::Y).unwrap(),
        );
        part.candidate_axis = Some(axis);
        cache.parts.insert(7, part);
        cache.advance_substep();
        assert_eq!(cache.candidate_axis, Some(axis));
        let mut part = cache.parts.remove(&7).unwrap();
        assert_eq!(part.candidate_axis, Some(axis));
        assert!(part.depth_axis.is_none() && part.contact.is_none());
        let crossing = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.5, 0.100002, -0.5)),
            DVec3::ZERO,
            -DVec3::Y * 0.000003,
            DVec3::ZERO,
        );
        for cache in [&mut cache, &mut part] {
            assert!(
                !certified_native_interval(&fixed, &triangle, &crossing, &cylinder, 1e-6, cache)
                    .unwrap()
            );
        }
        let hit = native_nonlinear_cast(&fixed, &triangle, &crossing, &cylinder)
            .unwrap()
            .unwrap();
        assert!(hit.time_of_impact > 0.0 && hit.time_of_impact < 1.0);
    }

    #[test]
    fn continuous_native_touching_retains_material_witnesses_and_normal() {
        use avian3d::parry::shape::{Cuboid, SupportMap, Triangle};
        let point = DVec3::new(0.0, -2.0, -2.0);
        let triangle = Triangle::new(point + DVec3::Z * 4.0, point + DVec3::Y * 4.0, point);
        let cuboid = Cuboid::new(DVec3::new(0.125, 0.25, 0.25));
        let normal = DVec3::new(1.0, -1.0, -1.0).normalize();
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let pose = Pose::from_translation(point - cuboid.local_support_point(-normal));
        assert_eq!(triangle.local_support_point(normal), point);
        assert_eq!(pose * cuboid.local_support_point(-normal), point);
        let touching = NonlinearRigidMotion::constant_position(pose);
        let contact =
            initial_native_contact(&fixed, &triangle, &touching, &cuboid, 10.0, normal).unwrap();
        assert_eq!(
            contact.point1, point,
            "native touching must retain the triangle's material point"
        );
        assert_eq!(
            contact.point2, point,
            "native touching must retain the cuboid's material point"
        );
        assert!(
            (contact.normal1 - normal).length() < 1e-15,
            "the true support direction owns the touching normal: {contact:?}"
        );
        assert_eq!(contact.dist, 0.0);
        use avian3d::parry::query::{
            epa::EPA,
            gjk::{CsoPoint, VoronoiSimplex},
        };
        let mut epa = EPA::new();
        let mut simplex = VoronoiSimplex::new();
        simplex.reset(CsoPoint::new(point, point));
        assert!(
            epa.closest_points(&pose, &triangle, &cuboid, &simplex)
                .is_none(),
            "a raw point without support metadata cannot invent a touching normal"
        );
        let support =
            CsoPoint::from_shapes(&pose, &triangle, &cuboid, normal).translate(DVec3::ZERO);
        assert_eq!(support.point, DVec3::ZERO);
        simplex.reset(support);
        let (first, second, direction) = epa
            .closest_points(&pose, &triangle, &cuboid, &simplex)
            .unwrap();
        assert_eq!(first, point);
        assert_eq!(second, point);
        assert!((direction - normal).length() < 1e-15);
    }

    #[test]
    fn continuous_scene_teardown_releases_collider_geometry_and_keeps_settings() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            PhysicsPlugins::default(),
            ContinuousCollisionPlugin::<()>::default(),
        ));
        let collider = Collider::sphere(1.0);
        let geometry = std::sync::Arc::downgrade(&collider.shape().0);
        app.world_mut()
            .resource_mut::<ContinuousContacts>()
            .shapes
            .push(Shape {
                entity: Entity::from_bits(1),
                part_index: 0,
                body: Entity::from_bits(2),
                collider,
                position: DVec3::ZERO,
                rotation: DQuat::IDENTITY,
                center: DVec3::ZERO,
            });
        app.world_mut()
            .resource_mut::<ContinuousCollisionDiagnostics>()
            .sweep_queries = 7;
        app.world_mut()
            .resource_mut::<ContinuousCollisionSettings>()
            .max_iterations = 333;
        assert!(geometry.upgrade().is_some());
        lunco_core::run_scene_teardown(app.world_mut());
        assert!(geometry.upgrade().is_none());
        assert!(
            app.world()
                .resource::<ContinuousContacts>()
                .shapes
                .is_empty()
        );
        assert_eq!(
            app.world()
                .resource::<ContinuousCollisionDiagnostics>()
                .sweep_queries,
            0
        );
        assert_eq!(
            app.world()
                .resource::<ContinuousCollisionSettings>()
                .max_iterations,
            333
        );
    }

    #[test]
    fn continuous_large_triangle_cylinder_edge_retains_geometry() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(
            DVec3::new(-250.0, 0.0, -250.0),
            DVec3::new(250.0, 0.0, 250.0),
            DVec3::new(250.0, 0.0, -250.0),
        );
        let cylinder = Cylinder::new(0.15, 0.4);
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let moving = NonlinearRigidMotion::constant_position(Pose::from_parts(
            DVec3::new(26.275517120869555, 0.3989396412921885, 26.459245381148524),
            DQuat::from_xyzw(
                0.10039221371651696,
                0.1742794765226331,
                0.6997061011501934,
                -0.6855358526530525,
            ),
        ));
        let axis = DVec3::new(
            -0.09186413013952333,
            0.3989396412921885,
            0.09186413013944517,
        );
        let contact = initial_native_contact(
            &fixed,
            &triangle,
            &moving,
            &cylinder,
            496.8394434529555,
            axis,
            &mut avian3d::parry::query::epa::EPA::new(),
        )
        .unwrap();
        validate_native_contact(&contact).unwrap();
        assert!(
            contact.dist > 0.0001 && contact.dist < 0.01,
            "the finite triangle edge is separated: {contact:?}"
        );
        assert!(contact.point1.y.abs() < 1e-9);
    }

    #[test]
    fn degenerate_native_triangle_retains_boundary_geometry() {
        use avian3d::parry::{
            query::PointQueryWithLocation,
            shape::{Triangle, TrianglePointLocation},
        };
        let triangle = Triangle::new(-DVec3::X, DVec3::X, DVec3::X * 2.0);
        for solid in [false, true] {
            let (projection, location) =
                triangle.project_local_point_and_get_location(DVec3::Y, solid);
            assert_eq!(projection.point, DVec3::ZERO);
            assert!(!projection.is_inside);
            assert!(matches!(location, TrianglePointLocation::OnEdge(0, _)));
        }
    }

    #[test]
    fn continuous_large_cuboid_cylinder_contact_retains_geometry() {
        use avian3d::parry::shape::{Cuboid, Cylinder};
        let cylinder = Cylinder::new(0.15, 0.4);
        let ground = Cuboid::new(DVec3::new(250.0, 0.5, 250.0));
        let wheel = NonlinearRigidMotion::constant_position(Pose::from_parts(
            DVec3::new(-9.896414017119408, 0.4000003863307968, 50.09619959068687),
            DQuat::from_xyzw(
                0.690498039592694,
                0.6902207244403379,
                -0.15237657279535471,
                0.1535876124767312,
            ),
        ));
        let fixed = NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::new(
            0.0, -0.5, 0.0,
        )));
        let contact = initial_native_contact(
            &wheel,
            &cylinder,
            &fixed,
            &ground,
            405.0532286425992,
            DVec3::new(9.896414017119408, -0.9000003863307968, -50.09619959068687),
            &mut avian3d::parry::query::epa::EPA::new(),
        )
        .unwrap();
        validate_native_contact(&contact).unwrap();
        assert!(
            contact.dist.abs() < 0.001,
            "near ground contact: {contact:?}"
        );
    }

    #[test]
    fn continuous_initial_query_rejects_missing_native_geometry() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(DVec3::ZERO, DVec3::X, DVec3::Z);
        let cylinder = Cylinder::new(0.1, 0.2);
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let distant =
            NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::X * 100.0));
        // Deliberately supply an incomplete internal prediction. The consumer
        // requires initial geometry, so a missing result must remain an error.
        let result = initial_native_contact(&fixed, &triangle, &distant, &cylinder, 0.1, DVec3::Y);
        assert!(
            result.is_err(),
            "missing initial geometry cannot classify clear space: {result:?}"
        );
    }

    #[test]
    fn continuous_native_radial_axis_retains_changed_trajectory() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let triangle = Triangle::new(
            DVec3::new(0.1, 0.0, 0.08),
            DVec3::new(0.12, 0.0, 0.08),
            DVec3::new(0.1, 0.0, 0.2),
        );
        let cylinder = Cylinder::new(0.1, 0.3);
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let pose = Pose::from_parts(
            DVec3::Y * 0.2999,
            DQuat::from_rotation_x(std::f64::consts::FRAC_PI_2),
        );
        let separating =
            NonlinearRigidMotion::new(pose, DVec3::ZERO, DVec3::Y * 0.00001, DVec3::ZERO);
        let mut cache = InitialContactCache::default();
        assert!(
            certified_native_interval(&fixed, &triangle, &separating, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        assert!(cache.candidate_axis.is_some());
        assert!(
            cache.contact.is_none(),
            "a full-shape radial proof needs no GJK contact"
        );
        let mut reverse = InitialContactCache::default();
        assert!(
            certified_native_interval(
                &separating,
                &cylinder,
                &fixed,
                &triangle,
                1e-6,
                &mut reverse,
            )
            .unwrap()
        );
        let approaching =
            NonlinearRigidMotion::new(pose, DVec3::ZERO, DVec3::X * 0.12, DVec3::ZERO);
        assert!(
            !certified_native_interval(
                &fixed,
                &triangle,
                &approaching,
                &cylinder,
                1e-6,
                &mut cache,
            )
            .unwrap()
        );
        let hit = native_nonlinear_cast(&fixed, &triangle, &approaching, &cylinder)
            .unwrap()
            .unwrap();
        assert!(
            hit.time_of_impact > 0.0 && hit.time_of_impact < 1.0,
            "{hit:?}"
        );
        assert!(hit.witness1.is_finite() && hit.witness2.is_finite());
    }

    #[test]
    fn continuous_native_finite_triangle_depth_certificate() {
        use avian3d::parry::shape::{Cylinder, Triangle};
        let floor = Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = Cylinder::new(0.1, 0.2);
        let fixed = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let start = Pose::from_translation(DVec3::new(-0.5, 0.099, -0.5));
        let touching = NonlinearRigidMotion::constant_position(start);
        let axis = finite_triangle_depth_axis(&fixed, &floor, &touching, &cylinder, 1e-6)
            .unwrap()
            .unwrap();
        assert_eq!(axis.axis, DVec3::Y);
        assert!((axis.lower_depth - 0.001).abs() < 1e-12);
        let contact = avian3d::parry::query::contact(&fixed.start, &floor, &start, &cylinder, 1.0)
            .unwrap()
            .unwrap();
        assert!((contact.dist + 0.001).abs() < 1e-6);
        assert_eq!(
            finite_triangle_depth_axis(&touching, &cylinder, &fixed, &floor, 1e-6).unwrap(),
            Some(DepthAxis {
                axis: -axis.axis,
                ..axis
            })
        );
        let mut cache = InitialContactCache::default();
        assert!(
            certified_native_interval(&fixed, &floor, &touching, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        assert!(
            cache.contact.is_none(),
            "finite-face proof must not fabricate a native contact"
        );
        let entering = NonlinearRigidMotion::new(start, DVec3::ZERO, -DVec3::Y * 0.5, DVec3::ZERO);
        assert!(
            !certified_native_interval(&fixed, &floor, &entering, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        let rotating =
            NonlinearRigidMotion::new(start, -DVec3::X * 10.0, DVec3::ZERO, -DVec3::Z * 0.2);
        assert!(
            !certified_native_interval(&fixed, &floor, &rotating, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        for center in [
            DVec3::new(0.0, 0.099, 0.0),
            DVec3::new(1.5, 0.099, 1.5),
            DVec3::new(-0.5, 0.0, -0.5),
        ] {
            let edge = NonlinearRigidMotion::constant_position(Pose::from_translation(center));
            // A real triangle point at the cylinder centre's projection proves
            // the depth even at an edge. A disjoint finite triangle declines.
            let result =
                finite_triangle_depth_axis(&fixed, &floor, &edge, &cylinder, 1e-6).unwrap();
            assert_eq!(result.is_some(), center.x != 1.5);
        }
        // A deep overlap can have a shorter escape through an edge, even when
        // the centre's projection lies inside the finite triangle.
        let wide = Cylinder::new(2.0, 0.49);
        let deep = NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::new(
            -0.5, 0.0, -0.5,
        )));
        assert!(
            finite_triangle_depth_axis(&fixed, &floor, &deep, &wide, 1e-6)
                .unwrap()
                .is_none()
        );
        // A small finite triangle can lie wholly inside the cap's projection.
        // Its highest vertex owns an interior ball even when the normal is
        // tilted and the cylinder's entire projection crosses every edge.
        let small = Triangle::new(
            DVec3::new(-0.02, 0.0, -0.02),
            DVec3::new(0.0, 0.001, 0.02),
            DVec3::new(0.02, 0.0, -0.02),
        );
        let centered =
            NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::Y * 0.099));
        let depth = finite_triangle_depth_axis(&fixed, &small, &centered, &cylinder, 1e-6)
            .unwrap()
            .unwrap();
        assert!((depth.lower_depth - 0.002).abs() < 1e-12);
        let native =
            avian3d::parry::query::contact(&fixed.start, &small, &centered.start, &cylinder, 1.0)
                .unwrap()
                .unwrap();
        assert!(
            (depth.lower_depth + native.dist).abs() <= 1e-6,
            "{depth:?} {native:?}"
        );
        let mut cache = InitialContactCache::default();
        assert!(
            certified_native_interval(&fixed, &small, &centered, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        assert!(cache.contact.is_none());
        let changed =
            NonlinearRigidMotion::new(centered.start, DVec3::ZERO, -DVec3::Y * 0.5, DVec3::ZERO);
        assert!(
            !certified_native_interval(&fixed, &small, &changed, &cylinder, 1e-6, &mut cache,)
                .unwrap()
        );
        let outside = NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::new(
            0.3, 0.099, 0.0,
        )));
        assert!(
            finite_triangle_depth_axis(&fixed, &small, &outside, &cylinder, 1e-6)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn continuous_native_queries_validate_geometry() {
        use avian3d::parry::{
            query::{DefaultQueryDispatcher, QueryDispatcher},
            shape::{Ball, Cylinder, HalfSpace, Triangle},
        };
        let triangle = Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = Cylinder::new(0.1, 0.2);

        assert!(
            DefaultQueryDispatcher
                .intersection_test(
                    &Pose::IDENTITY,
                    &Cylinder::new(50.0, 100.0),
                    &Cylinder::new(1.5, 1.0)
                )
                .unwrap(),
            "coincident overlapping cylinders must retain their intersection"
        );
        for angle in [0.0, 0.4, 1.2] {
            for center in [
                DVec3::new(-0.2, 0.099, -0.2),
                DVec3::new(0.1, 0.08, 0.1),
                DVec3::new(1.5, 0.099, 1.5),
                DVec3::new(-0.2, 0.25, -0.2),
            ] {
                let pose = Pose::new(center, DVec3::X * angle);
                let queries = CheckedNativeQueries::default();
                for swapped in [false, true] {
                    let (a, b, p): (
                        &dyn avian3d::parry::shape::Shape,
                        &dyn avian3d::parry::shape::Shape,
                        Pose,
                    ) = if swapped {
                        (&cylinder, &triangle, pose.inverse())
                    } else {
                        (&triangle, &cylinder, pose)
                    };
                    let reference = DefaultQueryDispatcher
                        .contact(&p, a, b, 10.0)
                        .unwrap()
                        .unwrap();
                    let contact = queries.contact(&p, a, b, 10.0).unwrap().unwrap();
                    let seeded =
                        avian3d::parry::query::details::contact_support_map_support_map_with_params(
                            &p,
                            a.as_support_map().unwrap(),
                            b.as_support_map().unwrap(),
                            10.0,
                            &mut avian3d::parry::query::gjk::VoronoiSimplex::new(),
                            Some(DVec3::Y),
                        );
                    let avian3d::parry::query::gjk::GJKResult::ClosestPoints(
                        point1,
                        point2,
                        normal,
                    ) = seeded
                    else {
                        panic!("native enclosing reference omitted geometry: {seeded:?}");
                    };
                    let initial = initial_native_contact(
                        &NonlinearRigidMotion::constant_position(Pose::IDENTITY),
                        a,
                        &NonlinearRigidMotion::constant_position(p),
                        b,
                        10.0,
                        DVec3::Y,
                    )
                    .unwrap();
                    assert_eq!(
                        initial.point1, point1,
                        "typed native support dispatch changed its witness"
                    );
                    assert_eq!(
                        initial.normal1, normal,
                        "typed native support dispatch changed its normal"
                    );
                    assert_eq!(
                        initial.dist,
                        (point2 - point1).dot(normal),
                        "typed native support dispatch changed its depth"
                    );
                    assert!((initial.point2 - point2).length() < 1e-12);
                    assert!(
                        (contact.dist - reference.dist).abs() < 1e-6,
                        "native adapter changed contact depth: {contact:?} vs {reference:?}"
                    );
                    assert!(contact.point1.is_finite() && contact.point2.is_finite());
                    assert!((contact.normal1.length_squared() - 1.0).abs() < 1e-12);
                    assert!(
                        ((p * contact.point2 - contact.point1).dot(contact.normal1) - contact.dist)
                            .abs()
                            < 1e-12
                    );
                    let reference_points = DefaultQueryDispatcher
                        .closest_points(&p, a, b, 10.0)
                        .unwrap();
                    let points = queries.closest_points(&p, a, b, 10.0).unwrap();
                    match (points, reference_points) {
                        (
                            avian3d::parry::query::ClosestPoints::Intersecting,
                            avian3d::parry::query::ClosestPoints::Intersecting,
                        ) => {}
                        (
                            avian3d::parry::query::ClosestPoints::WithinMargin(a1, b1),
                            avian3d::parry::query::ClosestPoints::WithinMargin(a2, b2),
                        ) => {
                            assert!(((p * b1 - a1).length() - (p * b2 - a2).length()).abs() < 1e-6)
                        }
                        result => {
                            panic!("native adapter changed closest-point geometry: {result:?}")
                        }
                    }
                }
                assert!(queries.failure.into_inner().unwrap().is_none());
            }
        }
        // The initial-contact cache uses world witnesses, while the native
        // dispatcher returns each witness in its own shape's local frame.
        let m1 = NonlinearRigidMotion::constant_position(Pose::new(
            DVec3::new(8.0, 2.0, -3.0),
            DVec3::Z * 0.7,
        ));
        let m2 = NonlinearRigidMotion::constant_position(
            m1.start * Pose::from_translation(DVec3::new(0.0, 0.3, 0.0)),
        );
        let ball = Ball::new(0.15);
        let reference = avian3d::parry::query::contact(&m1.start, &ball, &m2.start, &cylinder, 1.0)
            .unwrap()
            .unwrap();
        let contact = initial_native_contact(&m1, &ball, &m2, &cylinder, 1.0, DVec3::Y).unwrap();
        assert!((contact.point1 - reference.point1).length() < 1e-12);
        assert!((contact.point2 - reference.point2).length() < 1e-12);
        assert!((contact.dist - reference.dist).abs() < 1e-12);
        let mut invalid = contact;
        invalid.point2.x = f64::NAN;
        assert!(validate_native_contact(&invalid).is_err());
        invalid = contact;
        invalid.normal2 = DVec3::ZERO;
        assert!(validate_native_contact(&invalid).is_err());
        let plane = HalfSpace::new(DVec3::Y);
        let unsupported = CheckedNativeQueries::default();
        assert!(
            unsupported
                .closest_points(&Pose::IDENTITY, &plane, &plane, f64::MAX)
                .is_err()
        );
        assert!(unsupported.failure.into_inner().unwrap().is_some());
    }

    #[test]
    fn continuous_mesh_casts_obey_native_face_normals() {
        use avian3d::parry::shape::{SharedShape, TriMeshFlags};
        let mesh = SharedShape::trimesh_with_flags(
            vec![
                DVec3::new(-2.0, 0.0, -2.0),
                DVec3::new(2.0, 0.0, -2.0),
                DVec3::new(-2.0, 0.0, 2.0),
                DVec3::new(2.0, 0.0, 2.0),
            ],
            vec![[0, 2, 1], [1, 2, 3]],
            TriMeshFlags::FIX_INTERNAL_EDGES,
        )
        .unwrap();
        let floor = NonlinearRigidMotion::constant_position(Pose::IDENTITY);
        let floor_bounds = mesh.compute_local_aabb();
        let shape = Shape {
            entity: Entity::PLACEHOLDER,
            part_index: 0,
            body: Entity::PLACEHOLDER,
            collider: Collider::sphere(0.1),
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            center: DVec3::ZERO,
        };
        for side in [-1.0, 1.0] {
            let motion = NonlinearRigidMotion::new(
                Pose::from_translation(DVec3::new(0.2, 0.5 * side, 0.1)),
                DVec3::ZERO,
                DVec3::Y * -side,
                DVec3::ZERO,
            );
            let bounds = swept_bounds(&shape, &motion);
            let mut queries = 0;
            let hits = shape_hits(
                SweepMode::NonLinear,
                &floor,
                mesh.as_ref(),
                &floor_bounds,
                None,
                &motion,
                shape.collider.shape_scaled().as_ref(),
                &bounds,
                None,
                &mut queries,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap();
            if side < 0.0 {
                assert!(
                    hits.is_empty(),
                    "native oriented mesh backfaces cannot become contact planes"
                );
            } else {
                assert!(!hits.is_empty());
                for hit in hits {
                    assert!((hit.time_of_impact - 0.4).abs() < 1e-6);
                    assert!((hit.normal1 - DVec3::Y).length() < 1e-12);
                }
            }
        }
        let triangle = avian3d::parry::shape::Triangle::new(
            DVec3::new(-2.0, 0.0, -2.0),
            DVec3::new(-2.0, 0.0, 2.0),
            DVec3::new(2.0, 0.0, -2.0),
        );
        let cylinder = avian3d::parry::shape::Cylinder::new(0.1, 0.2);
        let sliding = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.2, 0.099, -0.2)),
            DVec3::ZERO,
            DVec3::X * 0.01,
            DVec3::Y * 0.01,
        );
        let mut initial_contact = InitialContactCache::default();
        assert!(
            certified_native_interval(
                &floor,
                &triangle,
                &sliding,
                &cylinder,
                1e-6,
                &mut initial_contact,
            )
            .unwrap(),
            "full cylinder supports certify an unchanged initial face depth while sliding and spinning"
        );
        let spinning = NonlinearRigidMotion {
            angvel: DVec3::Y * 2.0,
            ..sliding
        };
        let (initial, lower) = SupportInterval::new(&floor, &triangle, &spinning, &cylinder)
            .unwrap()
            .gap_bound(DVec3::Y)
            .unwrap();
        assert!(
            (initial - lower).abs() < 1e-12,
            "axial spin leaves the occupied cylinder unchanged"
        );
        assert!(
            certified_native_interval(
                &floor,
                &triangle,
                &spinning,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap()
        );
        let offset_spin = NonlinearRigidMotion {
            local_center: DVec3::X,
            angvel: DVec3::Z * 1.0,
            ..sliding
        };
        assert!(
            !certified_native_interval(
                &floor,
                &triangle,
                &offset_spin,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap(),
            "cylinder symmetry cannot hide COM-offset motion or a changing axis"
        );
        let tilting = NonlinearRigidMotion {
            angvel: DVec3::X * 2.0,
            ..sliding
        };
        assert!(
            !certified_native_interval(
                &floor,
                &triangle,
                &tilting,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap(),
            "a changing support feature that deepens initial contact must retain its cast"
        );
        let dropping = NonlinearRigidMotion {
            linvel: -DVec3::Y * 0.5,
            ..sliding
        };
        assert!(
            !certified_native_interval(
                &floor,
                &triangle,
                &dropping,
                &cylinder,
                1e-6,
                &mut initial_contact,
            )
            .unwrap(),
            "reusing immutable initial contact must still revalidate a changed endpoint trajectory"
        );
        let outside = NonlinearRigidMotion {
            start: Pose::from_translation(DVec3::new(1.5, 0.099, 1.5)),
            ..sliding
        };
        assert!(
            certified_native_interval(
                &floor,
                &triangle,
                &outside,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap(),
            "the finite triangle edge has a certified separating axis despite overlapping infinite face planes"
        );
        let approaching_edge = NonlinearRigidMotion {
            linvel: DVec3::new(-3.0, 0.0, -3.0),
            ..outside
        };
        assert!(
            !certified_native_interval(
                &floor,
                &triangle,
                &approaching_edge,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap(),
            "initial finite-edge separation cannot hide a future crossing"
        );
        let touching = NonlinearRigidMotion::new(
            Pose::from_translation(DVec3::new(-0.2, 0.1, -0.2)),
            DVec3::ZERO,
            DVec3::X * 0.1,
            DVec3::Z * 0.4,
        );
        let mut resting_queries = 0;
        assert!(
            shape_hits(
                SweepMode::NonLinear,
                &floor,
                mesh.as_ref(),
                &floor_bounds,
                None,
                &touching,
                shape.collider.shape_scaled().as_ref(),
                &swept_bounds(&shape, &touching),
                None,
                &mut resting_queries,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            resting_queries, 0,
            "ball symmetry and a whole-arc support gap certify rolling contact without a cast"
        );
        let hovering =
            NonlinearRigidMotion::constant_position(Pose::from_translation(DVec3::Y * 0.5));
        let mut queries = 0;
        assert!(
            shape_hits(
                SweepMode::NonLinear,
                &floor,
                mesh.as_ref(),
                &floor_bounds,
                None,
                &hovering,
                shape.collider.shape_scaled().as_ref(),
                &swept_bounds(&shape, &hovering),
                None,
                &mut queries,
                1e-6,
                &mut InitialContactCache::default(),
            )
            .unwrap()
            .is_empty()
        );
        assert_eq!(
            queries, 0,
            "a certified support gap must avoid the cast kernel"
        );
        let arc = NonlinearRigidMotion::new(
            hovering.start,
            -DVec3::X,
            DVec3::ZERO,
            -DVec3::Z * std::f64::consts::PI,
        );
        assert!((arc.position_at_time(1.0).translation.y - 0.5).abs() < 1e-12);
        let hits = shape_hits(
            SweepMode::NonLinear,
            &floor,
            mesh.as_ref(),
            &floor_bounds,
            None,
            &arc,
            shape.collider.shape_scaled().as_ref(),
            &swept_bounds(&shape, &arc),
            None,
            &mut queries,
            1e-6,
            &mut InitialContactCache::default(),
        )
        .unwrap();
        assert!(
            hits.iter()
                .any(|hit| hit.time_of_impact > 0.0 && hit.time_of_impact < 1.0),
            "clear endpoints cannot certify a rotating COM-offset shape's interior crossing"
        );
    }

    #[test]
    fn continuous_composite_keeps_each_part_impact() {
        use avian3d::parry::shape::{Ball, Compound, SharedShape};
        let owner = Entity::PLACEHOLDER;
        let rotation = DQuat::from_rotation_y(0.3);
        let position = DVec3::new(7.0, 2.0, -4.0);
        let composite = Compound::new(
            [0.0, 3.0, 1000.0]
                .map(|x| {
                    (
                        Pose::from_translation(DVec3::X * x),
                        SharedShape::cuboid(0.5, 5.0, 5.0),
                    )
                })
                .to_vec(),
        );
        let target = NonlinearRigidMotion::constant_position(Pose::from_parts(position, rotation));
        let sphere = Ball::new(0.1);
        let moving = NonlinearRigidMotion::new(
            Pose::from_parts(position + rotation * (DVec3::X * -0.6), rotation),
            DVec3::ZERO,
            rotation * (DVec3::X * 4.0),
            DVec3::ZERO,
        );
        let shape = Shape {
            entity: owner,
            part_index: 0,
            body: owner,
            collider: Collider::sphere(0.1),
            position: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            center: DVec3::ZERO,
        };
        let bounds = swept_bounds(&shape, &moving);
        let target_bounds = avian3d::parry::shape::Shape::compute_aabb(&composite, &target.start);
        let mut queries = 0;
        let hits = shape_hits(
            SweepMode::NonLinear,
            &target,
            &composite,
            &target_bounds,
            None,
            &moving,
            &sphere,
            &bounds,
            None,
            &mut queries,
            1e-6,
            &mut InitialContactCache::default(),
        )
        .unwrap();
        assert_eq!(
            queries, 2,
            "distant parts must be excluded by the whole-arc bound"
        );
        let hit = hits
            .iter()
            .find(|hit| hit.time_of_impact > 0.01)
            .expect("a later part crossing remains visible");
        assert!((hit.time_of_impact - 0.75).abs() < 1e-6);
        assert!(
            (hit.witness1.x - 2.5).abs() < 1e-6,
            "witness coordinates stay in the composite frame"
        );
        let mut reverse_queries = 0;
        let reverse = shape_hits(
            SweepMode::NonLinear,
            &moving,
            &sphere,
            &bounds,
            None,
            &target,
            &composite,
            &target_bounds,
            None,
            &mut reverse_queries,
            1e-6,
            &mut InitialContactCache::default(),
        )
        .unwrap();
        let reverse = reverse
            .iter()
            .find(|hit| hit.time_of_impact > 0.01)
            .unwrap();
        assert_eq!(queries, reverse_queries);
        assert!((hit.witness1 - reverse.witness2).length() < 1e-12);
        assert!((hit.normal1 - reverse.normal2).length() < 1e-12);
        let spinning =
            NonlinearRigidMotion::new(target.start, DVec3::ZERO, DVec3::ZERO, DVec3::Y * 0.02);
        let root_shape = Shape {
            collider: Collider::from(SharedShape::new(composite.clone())),
            ..shape
        };
        let spinning_bounds = swept_bounds(&root_shape, &spinning);
        let mut queries = 0;
        let selected = shape_hits(
            SweepMode::NonLinear,
            &spinning,
            &composite,
            &spinning_bounds,
            None,
            &moving,
            &sphere,
            &bounds,
            None,
            &mut queries,
            1e-6,
            &mut InitialContactCache::default(),
        )
        .unwrap();
        let mut exhaustive = Vec::new();
        for (pose, part) in composite.shapes() {
            if let Some(hit) = cast_shapes_nonlinear(
                &spinning.prepend(*pose),
                part.as_ref(),
                &moving,
                &sphere,
                0.0,
                1.0,
                false,
            )
            .unwrap()
            {
                exhaustive.push(hit.transform1_by(pose));
            }
        }
        assert_eq!(queries, 2);
        assert_eq!(selected.len(), exhaustive.len());
        for (selected, exhaustive) in selected.iter().zip(&exhaustive) {
            assert_eq!(selected.time_of_impact, exhaustive.time_of_impact);
            assert_eq!(selected.witness1, exhaustive.witness1);
        }
    }

    #[test]
    fn continuous_fault_refuses_native_pose_and_velocity_writeback() {
        let mut app = App::new();
        app.init_resource::<lunco_core::RuntimeFaults>()
            .add_plugins((
                MinimalPlugins,
                PhysicsPlugins::default()
                    .build()
                    .disable::<CcdPlugin>()
                    .disable::<avian3d::dynamics::solver::xpbd::XpbdSolverPlugin>(),
                crate::DeterministicJointSolverPlugin,
                ContinuousCollisionPlugin::<()>::default(),
            ));
        app.add_message::<AssetEvent<Mesh>>();
        app.init_resource::<Assets<Mesh>>();
        app.finish();
        app.cleanup();
        let position = DVec3::new(1.0, 2.0, 3.0);
        let velocity = DVec3::X * 10.0;
        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Collider::sphere(0.1),
                Position(position),
                LinearVelocity(velocity),
                crate::PhysicsOrderKey("body".into()),
            ))
            .id();
        app.update();
        app.world_mut()
            .resource_mut::<ContinuousCollisionSettings>()
            .max_iterations = 0;
        app.world_mut()
            .resource_mut::<Time<Physics>>()
            .advance_by(std::time::Duration::from_secs_f64(1.0 / 60.0));
        app.world_mut().run_schedule(PhysicsSchedule);
        assert!(app.world().resource::<lunco_core::RuntimeFaults>().active());
        assert!(
            app.world()
                .get::<SolverBody>(entity)
                .unwrap()
                .delta_position
                .length()
                > 0.0,
            "the native integrator prepared a changed pose"
        );
        assert_eq!(app.world().get::<Position>(entity).unwrap().0, position);
        assert_eq!(
            app.world().get::<LinearVelocity>(entity).unwrap().0,
            velocity
        );
    }

    #[test]
    fn continuous_native_revolute_coordinate_preserves_f64() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let joint = RevoluteJoint::new(a, b).with_hinge_axis(DVec3::Y);
        let rotation = DQuat::from_rotation_y(0.7);
        for angle in [0.0, 2e-10, -2e-10, 0.3, -0.3] {
            let (measured, axis) = crate::joint::revolute_coordinate(
                &joint,
                rotation,
                rotation * DQuat::from_rotation_y(angle),
            )
            .unwrap();
            assert!((measured - angle).abs() < 1e-15, "{measured} {angle}");
            assert!((axis - DVec3::Y).length() < 1e-15);
            for sign in [-1.0, 1.0] {
                let drifted = DQuat::from_rotation_y(angle) * (sign * (1.0 - 1e-12));
                assert!(
                    (rotation_displacement(DQuat::IDENTITY, drifted).unwrap() - DVec3::Y * angle)
                        .length()
                        < 1e-15
                );
            }
        }
    }

    #[test]
    fn continuous_swept_bvh_preserves_admitted_pairs_and_full_arcs() {
        let mut contacts = ContinuousContacts::default();
        let mut trajectories = Vec::new();
        for index in 0..8 {
            let shape = Shape {
                entity: Entity::PLACEHOLDER,
                part_index: index,
                body: Entity::PLACEHOLDER,
                collider: Collider::sphere(0.1),
                position: DVec3::X * index as f64,
                rotation: DQuat::IDENTITY,
                center: DVec3::X * index as f64,
            };
            let start = SolverBody::default();
            let mut end = SolverBody::default();
            if index == 0 {
                end.delta_position = DVec3::X * 2.0;
            }
            if index == 2 {
                end.delta_position = -DVec3::X * 2.0;
            }
            let mut shape = shape;
            if index == 4 {
                shape.position += DVec3::X * 2.0;
                end.delta_rotation = Rotation(DQuat::from_rotation_z(std::f64::consts::PI));
            }
            let motion = motion(&shape, &BodyMotion::new(&start, &end).unwrap());
            trajectories.push((motion, swept_bounds(&shape, &motion)));
            contacts.shapes.push(shape);
        }
        // Deliberately use pair order unrelated to BVH traversal or shape order.
        // Omitted overlapping pairs represent rejected hooks/layers/body filters.
        for shapes in [[4, 7], [2, 0], [0, 7], [4, 3], [4, 5], [0, 1]] {
            let pair = contacts.pairs.len();
            contacts
                .pair_indices
                .insert((shapes[0].min(shapes[1]), shapes[0].max(shapes[1])), pair);
            contacts.active_shapes.insert(shapes[0]);
            contacts.pairs.push(SweepPair {
                shapes,
                ccd: [Some(SweptCcd::default()), None],
                mode: SweepMode::NonLinear,
            });
        }
        let dense = contacts
            .pairs
            .iter()
            .enumerate()
            .filter_map(|(index, pair)| {
                trajectories[pair.shapes[0]]
                    .1
                    .intersects(&trajectories[pair.shapes[1]].1)
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let candidates = swept_pair_candidates(&mut contacts, &trajectories).unwrap();
        assert_eq!(candidates, dense);
        assert!(
            candidates.contains(&1),
            "both moving endpoints must be swept"
        );
        assert!(
            candidates.contains(&4),
            "an offset-COM arc must not be replaced by endpoint bounds"
        );
        assert!(!candidates.contains(&2), "a distant pair must be culled");
        // A corrected endpoint rebuilds admission, without stale candidate state.
        let shape = &contacts.shapes[0];
        let corrected = SolverBody {
            delta_position: DVec3::X * 7.0,
            ..default()
        };
        let m = motion(
            shape,
            &BodyMotion::new(&SolverBody::default(), &corrected).unwrap(),
        );
        trajectories[0] = (m, swept_bounds(shape, &m));
        assert!(
            swept_pair_candidates(&mut contacts, &trajectories)
                .unwrap()
                .contains(&2)
        );
        let restored = SolverBody {
            delta_position: DVec3::X * 2.0,
            ..default()
        };
        let shape = &contacts.shapes[0];
        let m = motion(
            shape,
            &BodyMotion::new(&SolverBody::default(), &restored).unwrap(),
        );
        trajectories[0] = (m, swept_bounds(shape, &m));
        assert_eq!(
            swept_pair_candidates(&mut contacts, &trajectories).unwrap(),
            dense,
            "native refitting must shrink bounds as well as expand them"
        );
        assert!(
            swept_pair_candidates(&mut ContinuousContacts::default(), &[])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn continuous_native_cylinder_projection_retains_minimum_distance_at_ties() {
        use avian3d::parry::{query::PointQuery, shape::Cylinder};
        let cylinder = Cylinder::new(0.1, 0.2);
        for (point, expected, inside) in [
            (DVec3::ZERO, DVec3::Y * 0.1, true),
            (DVec3::new(0.15, 0.0, 0.0), DVec3::new(0.2, 0.0, 0.0), true),
            (DVec3::new(0.1, 0.0, 0.0), DVec3::new(0.1, 0.1, 0.0), true),
            (-DVec3::Y * 0.02, -DVec3::Y * 0.1, true),
            (DVec3::new(0.3, 0.2, 0.0), DVec3::new(0.2, 0.1, 0.0), false),
        ] {
            let projected = cylinder.project_local_point(point, false);
            assert_eq!(projected.is_inside, inside);
            assert!(
                (projected.point - expected).length() < 1e-14,
                "{point:?}: {projected:?}"
            );
            let pose = Pose::from_parts(DVec3::new(3.0, -2.0, 5.0), DQuat::from_rotation_z(0.4));
            let transformed = cylinder.project_point(&pose, pose * point, false);
            // Transform rounding may choose either equally near feature.
            assert!(
                ((transformed.point - pose * point).length() - (expected - point).length()).abs()
                    < 1e-13
            );
            let local = pose.inverse_transform_point(transformed.point);
            assert!(
                (local.y.abs() - cylinder.half_height).abs() < 1e-13
                    || (DVec3::new(local.x, 0.0, local.z).length() - cylinder.radius).abs() < 1e-13
            );
        }
    }

    #[test]
    fn continuous_constraint_math_covers_multiple_contacts_arcs_and_com() {
        ComputeTaskPool::get_or_init(|| bevy::tasks::TaskPoolBuilder::new().num_threads(1).build());
        let mut world = World::new();
        let mut schedule = Schedule::new(PhysicsJointPositionSchedule);
        schedule.add_systems(|| {});
        world.add_schedule(schedule);
        world.insert_resource(ContinuousCollisionSettings::default());
        world.insert_resource(ContinuousCollisionDiagnostics::default());
        world.insert_resource(ContactGraph::default());
        world.insert_resource(ConstraintGraph::default());
        let mut clock = Time::<Substeps>::default();
        clock.advance_by(std::time::Duration::from_secs(1));
        world.insert_resource(clock);
        world.insert_resource(lunco_core::RuntimeFaults::default());
        world.insert_resource(PhysicsHolds::default());
        let inertia =
            SolverBodyInertia::new(1.0, SymmetricMatrix::ZERO, LockedAxes::new(), 0, true);
        let a = world
            .spawn((
                SolverBody {
                    delta_position: DVec3::X * 2.0,
                    ..default()
                },
                inertia.clone(),
            ))
            .id();
        let b = world
            .spawn((
                SolverBody {
                    delta_position: DVec3::X * 4.0,
                    ..default()
                },
                inertia,
            ))
            .id();
        let wall = world.spawn_empty().id();
        let shape = |entity, center, collider| Shape {
            entity,
            part_index: 0,
            body: entity,
            collider,
            center,
            position: center,
            rotation: DQuat::IDENTITY,
        };
        let sa = shape(a, DVec3::X * 2.0, Collider::sphere(0.1));
        let sb = shape(b, DVec3::ZERO, Collider::sphere(0.1));
        let sw = shape(wall, DVec3::X * 3.0, Collider::cuboid(1.0, 10.0, 10.0));
        let mut contacts = ContinuousContacts::default();
        contacts.starts.insert(a, SolverBody::default());
        contacts.starts.insert(b, SolverBody::default());
        contacts.shapes = vec![sa.clone(), sb, sw];
        for shapes in [[0, 1], [0, 2], [1, 2]] {
            let pair = contacts.pairs.len();
            contacts.pair_indices.insert((shapes[0], shapes[1]), pair);
            contacts.active_shapes.insert(shapes[0]);
            contacts.pairs.push(SweepPair {
                shapes,
                ccd: [Some(SweptCcd::default()), None],
                mode: SweepMode::NonLinear,
            });
        }
        world.init_resource::<JointSolverOrder>();
        world.init_resource::<PhysicsJointPositionScope>();
        world.insert_resource(contacts);
        solve_contacts(&mut world);
        assert!(
            !world.resource::<lunco_core::RuntimeFaults>().active(),
            "{:?}",
            world.resource::<lunco_core::RuntimeFaults>()
        );
        let ax = 2.0 + world.get::<SolverBody>(a).unwrap().delta_position.x;
        let bx = world.get::<SolverBody>(b).unwrap().delta_position.x;
        assert!(ax <= 2.4 + 1e-6 && bx <= ax - 0.2 + 1e-6, "{ax} {bx}");

        // Both endpoints are outside this plane; the middle of the arc crosses it.
        let s = SolverBody::default();
        let e = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_z(std::f64::consts::PI)),
            ..default()
        };
        let p = Plane {
            collider1: Entity::PLACEHOLDER,
            collider2: Entity::PLACEHOLDER,
            part1: 0,
            part2: 0,
            body1: a,
            body2: wall,
            center1: DVec3::ZERO,
            center2: DVec3::Y * 0.5,
            point1: DVec3::X,
            point2: DVec3::ZERO,
            normal: DVec3::Y,
            offset: 0.0,
            native_point: None,
        };
        let (t, g) = minimum_gap(&p, &s, &e, &s, &s, 1e-6).unwrap();
        assert!((t - 0.5).abs() < 1e-3 && (g + 0.5).abs() < 1e-6, "{t} {g}");
        let mut invalid = p.clone();
        invalid.normal = DVec3::splat(f64::NAN);
        assert!(minimum_gap(&invalid, &s, &e, &s, &s, 1e-6).is_err());
        // Verify the exact derivative of the interpolated rotation with respect
        // to a left perturbation of the solved endpoint, including a nonzero start.
        let s = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_y(0.2)),
            ..default()
        };
        let e = SolverBody {
            delta_rotation: Rotation(DQuat::from_rotation_z(0.7) * s.delta_rotation.0),
            ..default()
        };
        let t = 0.37;
        let witness_motion = WitnessMotion::new(&s, &e, DVec3::X).unwrap();
        let (r, _) = witness_motion.position_and_velocity(t);
        let analytic = witness_motion.gradient(t, DVec3::Y);
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let mut perturbed = e.clone();
            perturbed.delta_rotation.0 = DQuat::from_scaled_axis(axis * 1e-7) * e.delta_rotation.0;
            let numerical = (WitnessMotion::new(&s, &perturbed, DVec3::X)
                .unwrap()
                .position_and_velocity(t)
                .0
                .dot(DVec3::Y)
                - r.dot(DVec3::Y))
                / 1e-7;
            assert!(
                (numerical - analytic.dot(axis)).abs() < 1e-7,
                "{numerical} {analytic:?}"
            );
        }
        // The sweep rotation pivots about the COM, even for an offset collider.
        let mut sa = sa;
        sa.position = sa.center + DVec3::X;
        sa.rotation = DQuat::from_rotation_y(0.3);
        let motion = motion(&sa, &BodyMotion::new(&s, &e).unwrap());
        let expected =
            sa.center + e.delta_position + e.delta_rotation.0 * (sa.position - sa.center);
        assert!((motion.position_at_time(1.0).translation - expected).length() < 1e-12);
        let bounds = swept_bounds(&sa, &motion);
        for step in 0..=100 {
            let pose = motion.position_at_time(step as f64 / 100.0);
            assert!(bounds.contains(&sa.collider.shape_scaled().compute_aabb(&pose)));
        }
    }
}

impl lunco_core::SceneResource for ContinuousContacts {}

impl lunco_core::SceneResource for ContinuousCollisionDiagnostics {}
