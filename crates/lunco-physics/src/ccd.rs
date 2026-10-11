//! Continuous contact constraints share the native substep joint solve.

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
    schedule::{SolverSystems, SubstepSolverSystems},
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{PositionConstraint, XpbdSolverSystems},
};
use avian3d::math::rotation_vector;
use avian3d::parry::query::details::{NormalConstraints, NormalConstraintsPair};
use avian3d::parry::{
    bounding_volume::{Aabb, BoundingVolume},
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
    prelude::*,
};
use std::collections::HashMap;
use std::marker::PhantomData;

/// Deterministic convergence bounds for continuous contact and joint constraints.
#[derive(Resource, Reflect, Clone, Debug)]
#[reflect(Resource)]
pub struct ContinuousCollisionSettings {
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
            .init_resource::<ContinuousCollisionDiagnostics>()
            .init_resource::<ContinuousContacts>()
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
            )
            .configure_sets(
                SubstepSchedule,
                (
                    XpbdSolverSystems::VelocityProjection,
                    SubstepSolverSystems::Relax,
                )
                    .run_if(no_fault),
            )
            .configure_sets(
                PhysicsSchedule,
                (
                    SolverSystems::PostSubstep,
                    SolverSystems::Restitution,
                    SolverSystems::Finalize,
                    SolverSystems::StoreContactImpulses,
                )
                    .run_if(no_fault),
            );
    }
}
fn no_fault(faults: Res<lunco_core::RuntimeFaults>) -> bool {
    !faults.active()
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
    ccd: Option<SweptCcd>,
}
#[derive(Resource, Default)]
struct ContinuousContacts {
    pairs: Vec<(Shape, Shape, SweepMode)>,
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
                contacts.pairs.push((
                    Shape {
                        entity: e1,
                        part_index: part1,
                        body: o1.body,
                        collider: collider1.clone(),
                        position: *position1,
                        rotation: *rotation1,
                        center: bp1.0 + br1.0 * com1.0,
                        ccd: ccd1.copied(),
                    },
                    Shape {
                        entity: e2,
                        part_index: part2,
                        body: o2.body,
                        collider: collider2.clone(),
                        position: *position2,
                        rotation: *rotation2,
                        center: bp2.0 + br2.0 * com2.0,
                        ccd: ccd2.copied(),
                    },
                    mode,
                ));
            }
        }
    }
}
fn capture_start(bodies: Query<(Entity, &SolverBody)>, mut contacts: ResMut<ContinuousContacts>) {
    contacts.starts.clear();
    contacts
        .starts
        .extend(bodies.iter().map(|(e, b)| (e, b.clone())));
}
fn motion(shape: &Shape, start: &SolverBody, end: &SolverBody) -> NonlinearRigidMotion {
    let center_local = shape.rotation.inverse() * (shape.center - shape.position);
    let pose = Pose::from_parts(
        shape.center
            + start.delta_position
            + start.delta_rotation.0 * (shape.position - shape.center),
        start.delta_rotation.0 * shape.rotation,
    );
    NonlinearRigidMotion::new(
        pose,
        center_local,
        end.delta_position - start.delta_position,
        rotation_vector(end.delta_rotation.0 * start.delta_rotation.0.inverse()),
    )
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

/// Certify the full-shape support gap along a fixed world axis throughout
/// the actual COM motion, including its rotation and changing support features.
fn support_gap_bound(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    axis: DVec3,
) -> Result<Option<(f64, f64)>, String> {
    let (Some(support1), Some(support2)) = (s1.as_support_map(), s2.as_support_map()) else {
        return Ok(None);
    };
    let acceleration = |shape: &dyn avian3d::parry::shape::Shape, motion: &NonlinearRigidMotion| {
        let sphere = shape.compute_local_bounding_sphere();
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
            sphere.radius * perpendicular * (perpendicular + motion.angvel.dot(cylinder_axis).abs())
        } else {
            sphere.radius * motion.angvel.length() * motion.angvel.cross(axis).length()
        };
        center + geometry
    };
    let acceleration = acceleration(s1, m1) + acceleration(s2, m2);
    let mut initial = 0.0;
    let mut lower = f64::INFINITY;
    for t in [0.0, 1.0] {
        let pose1 = m1.position_at_time(t);
        let pose2 = m2.position_at_time(t);
        let max1 = support1.support_point(&pose1, axis).dot(axis);
        let min2 = support2.support_point(&pose2, -axis).dot(axis);
        let gap = min2 - max1;
        if t == 0.0 {
            initial = gap;
        }
        lower = lower.min(gap);
    }
    if !acceleration.is_finite() || !initial.is_finite() || !lower.is_finite() {
        return Err("continuous separating-axis bound exceeded its finite numeric range".into());
    }
    // Every material-point pair follows a chord with this projected
    // second-derivative remainder. Supports include all feature changes.
    Ok(Some((initial, lower - acceleration / 8.0)))
}
fn separated_on_triangle_axis(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    tolerance: f64,
) -> Result<bool, String> {
    let axis = if let Some(triangle) = s1.as_triangle() {
        triangle.normal().map(|n| m1.start.rotation * n)
    } else if let Some(triangle) = s2.as_triangle() {
        triangle.normal().map(|n| m2.start.rotation * n)
    } else {
        None
    };
    let Some(axis) = axis else {
        return Ok(false);
    };
    for direction in [axis, -axis] {
        if support_gap_bound(m1, s1, m2, s2, direction)?
            .is_some_and(|(_, lower)| lower >= -tolerance)
        {
            return Ok(true);
        }
    }
    Ok(false)
}
/// Immutable starting geometry is shared by all closure sweeps in one substep.
/// Native composite part identities isolate each leaf query.
#[derive(Default)]
struct InitialContactCache {
    contact: Option<Option<avian3d::parry::query::Contact>>,
    parts: HashMap<u32, InitialContactCache>,
}

/// A native closest-contact axis can certify separation throughout the arc,
/// or certify that an initial contact's exact support depth cannot increase.
/// Each convex leaf is checked independently, including finite mesh features.
fn certified_native_axis(
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
    let sphere1 = s1.compute_local_bounding_sphere();
    let sphere2 = s2.compute_local_bounding_sphere();
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
    if axis.is_finite()
        && axis.length_squared() > 0.0
        && support_gap_bound(m1, s1, m2, s2, axis.normalize())?
            .is_some_and(|(_, lower)| lower >= -tolerance)
    {
        return Ok(true);
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
    let Some(contact) = contact else {
        return Ok(false);
    };
    if !contact.dist.is_finite()
        || !contact.normal1.is_finite()
        || (contact.normal1.length_squared() - 1.0).abs() > 1e-6
    {
        return Err("continuous initial contact returned invalid native geometry".into());
    }
    let Some((initial, lower)) = support_gap_bound(m1, s1, m2, s2, contact.normal1)? else {
        return Ok(false);
    };
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
fn initial_native_contact(
    m1: &NonlinearRigidMotion,
    s1: &dyn avian3d::parry::shape::Shape,
    m2: &NonlinearRigidMotion,
    s2: &dyn avian3d::parry::shape::Shape,
    prediction: f64,
    axis: DVec3,
) -> Result<Option<avian3d::parry::query::Contact>, String> {
    use avian3d::parry::query::{
        Contact,
        details::contact_support_map_support_map_with_params,
        gjk::{GJKResult, VoronoiSimplex},
    };
    // Balls have a specialized native kernel. All other convex triangle pairs
    // use this same support-map kernel in the native default dispatcher.
    if (s1.as_triangle().is_some() || s2.as_triangle().is_some())
        && s1.as_ball().is_none()
        && s2.as_ball().is_none()
        && let (Some(support1), Some(support2)) = (s1.as_support_map(), s2.as_support_map())
    {
        let relative = m1.start.inverse() * m2.start;
        let direction = (axis.is_finite() && axis.length_squared() > 0.0)
            .then(|| m1.start.rotation.inverse() * axis.normalize());
        let result = contact_support_map_support_map_with_params(
            &relative,
            support1,
            support2,
            prediction,
            &mut VoronoiSimplex::new(),
            direction,
        );
        return match result {
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
                Ok(Some(contact))
            }
            GJKResult::NoIntersection(_) => Ok(None),
            _ => Err("native continuous initial contact did not return contact geometry".into()),
        };
    }
    avian3d::parry::query::contact(&m1.start, s1, &m2.start, s2, prediction)
        .map_err(|error| format!("unsupported continuous initial contact: {error:?}"))
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
                .map_err(|error| format!("unsupported continuous composite part: {error:?}"))?;
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
    if mode == SweepMode::NonLinear
        && (separated_on_triangle_axis(m1, s1, m2, s2, tolerance)?
            || certified_native_axis(m1, s1, m2, s2, tolerance, cache)?)
    {
        return Ok(Vec::new());
    }
    *queries += 1;
    let hit = if mode == SweepMode::NonLinear {
        cast_shapes_nonlinear(m1, s1, m2, s2, 0.0, 1.0, false)
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
    };
    let Some(mut hit) = hit.map_err(|error| format!("unsupported continuous shapes: {error:?}"))?
    else {
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
    contacts: &ContinuousContacts,
    tolerance: f64,
    diagnostics: &mut ContinuousCollisionDiagnostics,
    initial_contacts: &mut [InitialContactCache],
) -> Result<Vec<Plane>, String> {
    let begin = bevy::platform::time::Instant::now();
    let result = sweep_geometry(world, contacts, tolerance, diagnostics, initial_contacts);
    diagnostics.sweep_time += begin.elapsed();
    result
}
fn sweep_geometry(
    world: &World,
    contacts: &ContinuousContacts,
    tolerance: f64,
    diagnostics: &mut ContinuousCollisionDiagnostics,
    initial_contacts: &mut [InitialContactCache],
) -> Result<Vec<Plane>, String> {
    // The native contact graph is prepared once per physics step. Keep newly
    // admitted impact witnesses through its remaining substeps so zero-time
    // touching cannot lose the contact before the next native preparation.
    let mut planes = Vec::new();
    let dummy = SolverBody::default();
    // This World is immutable for one sweep. Reuse each leaf's exact motion
    // and bound across candidate pairs, then rebuild after any correction.
    let mut trajectories = HashMap::new();
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
    for (pair_index, (a, b, mode)) in contacts.pairs.iter().enumerate() {
        let s1 = contacts.starts.get(&a.body).unwrap_or(&dummy);
        let s2 = contacts.starts.get(&b.body).unwrap_or(&dummy);
        let e1 = world.get::<SolverBody>(a.body).unwrap_or(&dummy);
        let e2 = world.get::<SolverBody>(b.body).unwrap_or(&dummy);
        let (m1, bounds1) = *trajectories
            .entry((a.entity, a.part_index))
            .or_insert_with(|| {
                let motion = motion(a, s1, e1);
                let bounds = swept_bounds(a, &motion);
                (motion, bounds)
            });
        let (m2, bounds2) = *trajectories
            .entry((b.entity, b.part_index))
            .or_insert_with(|| {
                let motion = motion(b, s2, e2);
                let bounds = swept_bounds(b, &motion);
                (motion, bounds)
            });
        // Endpoint quaternions do not retain complete revolutions. Refuse an
        // ambiguous integration increment before the logarithm can hide it.
        if [e1, e2]
            .into_iter()
            .any(|body| body.angular_velocity.length() * dt >= std::f64::consts::PI)
        {
            return Err("continuous collision requires an angular integration increment below pi radians per substep".into());
        }
        let linear = (m2.linvel - m1.linvel).length() / dt;
        let angular = (m2.angvel - m1.angvel).length() / dt;
        if ![a.ccd, b.ccd]
            .into_iter()
            .flatten()
            .any(|ccd| linear >= ccd.linear_threshold || angular >= ccd.angular_threshold)
        {
            continue;
        }
        if !bounds1.intersects(&bounds2) {
            continue;
        }
        let hits = shape_hits(
            *mode,
            &m1,
            a.collider.shape_scaled().as_ref(),
            &bounds1,
            None,
            &m2,
            b.collider.shape_scaled().as_ref(),
            &bounds2,
            None,
            &mut diagnostics.sweep_queries,
            tolerance,
            &mut initial_contacts[pair_index],
        )
        .map_err(|detail| format!("continuous sweep {:?} / {:?}: {detail}", a.entity, b.entity))?;
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
                point1: a.position + a.rotation * hit.witness1 - a.center,
                point2: b.position + b.rotation * hit.witness2 - b.center,
                normal,
                offset: 0.0,
                native_point: None,
            };
            let (_, gap) = minimum_gap(&plane, s1, e1, s2, e2, tolerance)?;
            if gap < -tolerance {
                planes.push(plane);
            }
        }
    }
    Ok(planes)
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
    fn new(start: &SolverBody, end: &SolverBody, point: DVec3) -> Self {
        Self {
            start: start.delta_position,
            end: end.delta_position,
            omega: rotation_vector(end.delta_rotation.0 * start.delta_rotation.0.inverse()),
            point: start.delta_rotation.0 * point,
        }
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
    let motion1 = WitnessMotion::new(s1, e1, p.point1);
    let motion2 = WitnessMotion::new(s2, e2, p.point2);
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
            (p.center2 + r2 - p.center1 - r1).dot(p.normal) + p.offset,
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
    let a1 = WitnessMotion::new(s1, &b1, plane.point1).gradient(t, plane.normal);
    let a2 = WitnessMotion::new(s2, &b2, plane.point2).gradient(t, plane.normal);
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
        let mut initial_contacts = (0..contacts.pairs.len())
            .map(|_| InitialContactCache::default())
            .collect::<Vec<_>>();
        let mut planes = sweep(
            world,
            &contacts,
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
                    rotation_vector(after.delta_rotation.0 * before.delta_rotation.0.inverse())
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
                    &contacts,
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
            let separation = |time| {
                let a = WitnessMotion::new(s1, e1, plane.point1)
                    .position_and_velocity(time)
                    .0;
                let b = WitnessMotion::new(s2, e2, plane.point2)
                    .position_and_velocity(time)
                    .0;
                (plane.center2 + b - plane.center1 - a).dot(plane.normal) + plane.offset
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
            ccd: None,
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
            certified_native_axis(
                &floor,
                &triangle,
                &sliding,
                &cylinder,
                1e-6,
                &mut initial_contact
            )
            .unwrap(),
            "full cylinder supports certify an unchanged initial face depth while sliding and spinning"
        );
        let spinning = NonlinearRigidMotion {
            angvel: DVec3::Y * 2.0,
            ..sliding
        };
        let (initial, lower) = support_gap_bound(&floor, &triangle, &spinning, &cylinder, DVec3::Y)
            .unwrap()
            .unwrap();
        assert!(
            (initial - lower).abs() < 1e-12,
            "axial spin leaves the occupied cylinder unchanged"
        );
        assert!(
            certified_native_axis(
                &floor,
                &triangle,
                &spinning,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default()
            )
            .unwrap()
        );
        let offset_spin = NonlinearRigidMotion {
            local_center: DVec3::X,
            angvel: DVec3::Z * 1.0,
            ..sliding
        };
        assert!(
            !certified_native_axis(
                &floor,
                &triangle,
                &offset_spin,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default()
            )
            .unwrap(),
            "cylinder symmetry cannot hide COM-offset motion or a changing axis"
        );
        let tilting = NonlinearRigidMotion {
            angvel: DVec3::X * 0.3,
            ..sliding
        };
        assert!(
            !certified_native_axis(
                &floor,
                &triangle,
                &tilting,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default()
            )
            .unwrap(),
            "a changing support feature that deepens initial contact must retain its cast"
        );
        let dropping = NonlinearRigidMotion {
            linvel: -DVec3::Y * 0.001,
            ..sliding
        };
        let cached = initial_contact.contact;
        assert!(cached.is_some());
        assert!(
            !certified_native_axis(
                &floor,
                &triangle,
                &dropping,
                &cylinder,
                1e-6,
                &mut initial_contact
            )
            .unwrap(),
            "reusing immutable initial contact must still revalidate a changed endpoint trajectory"
        );
        assert_eq!(initial_contact.contact, cached);
        let outside = NonlinearRigidMotion {
            start: Pose::from_translation(DVec3::new(1.5, 0.099, 1.5)),
            ..sliding
        };
        assert!(
            certified_native_axis(
                &floor,
                &triangle,
                &outside,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default()
            )
            .unwrap(),
            "the finite triangle edge has a certified separating axis despite overlapping infinite face planes"
        );
        let approaching_edge = NonlinearRigidMotion {
            linvel: DVec3::new(-3.0, 0.0, -3.0),
            ..outside
        };
        assert!(
            !certified_native_axis(
                &floor,
                &triangle,
                &approaching_edge,
                &cylinder,
                1e-6,
                &mut InitialContactCache::default()
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
                &mut InitialContactCache::default()
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
                &mut InitialContactCache::default()
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
            ccd: None,
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
                PhysicsPlugins::default().build().disable::<CcdPlugin>(),
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
            let (measured, axis) = joint
                .angle_and_axis(rotation, rotation * DQuat::from_rotation_y(angle))
                .unwrap();
            assert!((measured - angle).abs() < 1e-15, "{measured} {angle}");
            assert!((axis - DVec3::Y).length() < 1e-15);
            for sign in [-1.0, 1.0] {
                let drifted = DQuat::from_rotation_y(angle) * (sign * (1.0 - 1e-12));
                assert!((rotation_vector(drifted) - DVec3::Y * angle).length() < 1e-15);
            }
        }
    }

    #[test]
    fn continuous_native_damping_preserves_free_locked_axes() {
        let mut fixed = SolverBody::DUMMY;
        let mut body = SolverBody {
            angular_velocity: DVec3::Y * 2.0,
            ..default()
        };
        let inertia = SolverBodyInertia::new(
            1.0,
            SymmetricMatrix::IDENTITY,
            LockedAxes::new().lock_rotation_x().lock_rotation_z(),
            0,
            true,
        );
        avian3d::dynamics::solver::apply_joint_damping(
            [&mut fixed, &mut body],
            [&SolverBodyInertia::DUMMY, &inertia],
            JointDamping {
                linear: 0.0,
                angular: 20.0,
            },
            1.0 / 480.0,
        );
        assert!((body.angular_velocity.y - 2.0 * (1.0 - 20.0 / 480.0)).abs() < 1e-12);
        assert_eq!(fixed.angular_velocity, DVec3::ZERO);
        assert_eq!(body.angular_velocity.x, 0.0);
        assert_eq!(body.angular_velocity.z, 0.0);
    }

    #[test]
    fn continuous_hard_joint_pass_preserves_soft_channels() {
        use avian3d::dynamics::solver::xpbd::{HardJointConstraint, joints::FixedJointSolverData};
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut joint = FixedJoint::new(a, b);
        joint.point_compliance = 1.0;
        joint.angle_compliance = 1.0;
        let mut data = FixedJointSolverData::default();
        let inertia =
            SolverBodyInertia::new(1.0, SymmetricMatrix::IDENTITY, LockedAxes::new(), 0, true);
        let mut first = SolverBody::default();
        let mut second = SolverBody {
            delta_position: DVec3::X,
            delta_rotation: Rotation(DQuat::from_rotation_y(0.2)),
            ..default()
        };
        let before = second.clone();
        joint.solve_hard_constraints(
            [&mut first, &mut second],
            [&inertia, &inertia],
            &mut data,
            1.0 / 480.0,
        );
        assert_eq!(first.delta_position, DVec3::ZERO);
        assert_eq!(second.delta_position, before.delta_position);
        assert_eq!(second.delta_rotation, before.delta_rotation);
        joint.angle_compliance = 0.0;
        joint.solve_hard_constraints(
            [&mut first, &mut second],
            [&inertia, &inertia],
            &mut data,
            1.0 / 480.0,
        );
        assert_eq!(first.delta_position, DVec3::ZERO);
        assert_eq!(
            second.delta_position, before.delta_position,
            "a hard rotation must not stiffen soft translation"
        );
        assert!(
            second
                .delta_rotation
                .0
                .angle_between(first.delta_rotation.0)
                < 0.01,
            "the hard angular channel was solved"
        );
    }

    #[test]
    fn continuous_constraint_math_covers_multiple_contacts_arcs_and_com() {
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
            ccd: Some(SweptCcd::default()),
        };
        let sa = shape(a, DVec3::X * 2.0, Collider::sphere(0.1));
        let sb = shape(b, DVec3::ZERO, Collider::sphere(0.1));
        let sw = shape(wall, DVec3::X * 3.0, Collider::cuboid(1.0, 10.0, 10.0));
        let mut contacts = ContinuousContacts::default();
        contacts.starts.insert(a, SolverBody::default());
        contacts.starts.insert(b, SolverBody::default());
        contacts.pairs = vec![
            (sa.clone(), sb.clone(), SweepMode::NonLinear),
            (sa.clone(), sw.clone(), SweepMode::NonLinear),
            (sb, sw.clone(), SweepMode::NonLinear),
        ];
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
        let witness_motion = WitnessMotion::new(&s, &e, DVec3::X);
        let (r, _) = witness_motion.position_and_velocity(t);
        let analytic = witness_motion.gradient(t, DVec3::Y);
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let mut perturbed = e.clone();
            perturbed.delta_rotation.0 = DQuat::from_scaled_axis(axis * 1e-7) * e.delta_rotation.0;
            let numerical = (WitnessMotion::new(&s, &perturbed, DVec3::X)
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
        let motion = motion(&sa, &s, &e);
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
