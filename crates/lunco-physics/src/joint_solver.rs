//! Stable ordering for XPBD constraints that mutate shared solver bodies.

use lunco_core::AppSceneExt;

use std::time::Duration;

use crate::joint_projection::{JointDrive, JointProjection, ProjectionJoint};
use crate::order::{ordered_physics_entities, report_invalid_physics_order};
use crate::{PhysicsHolds, PhysicsOrderKey};
use avian3d::dynamics::integrator::IntegrationSystems;
use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    schedule::{SolverSystems, SubstepSolverSystems},
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{
        XpbdConstraint, XpbdConstraintSolverData, XpbdSolverPlugin, XpbdSolverSystems, joints::*,
        prepare_xpbd_joint,
    },
};
use avian3d::physics_transform::{PreSolveDeltaPosition, PreSolveDeltaRotation};
use avian3d::prelude::*;
use bevy::ecs::component::Mutable;
use bevy::ecs::schedule::{ScheduleCleanupPolicy, ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::*;

/// Ordered application extension points around the native XPBD joint passes.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhysicsJointSolvePass {
    /// Fixed joints.
    Fixed,
    /// Application constraints that must run after fixed joints and before revolute joints.
    BeforeRevolute,
    /// Revolute joints.
    Revolute,
    /// Spherical joints.
    Spherical,
    /// Prismatic joints.
    Prismatic,
    /// Distance joints.
    Distance,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PhysicsJointOrderPreparation;

/// Position-only joint correction pass reused by continuous contact solving.
/// Motors are integrated exclusively by the ordinary substep pass.
#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct PhysicsJointPositionSchedule;

/// Ordered position-only sweeps after the single motor-integration pass.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct JointPositionSweep(u8);

#[derive(Resource, Default)]
pub(crate) struct JointSolverOrder {
    pub(crate) fixed: Vec<Entity>,
    pub(crate) revolute: Vec<Entity>,
    pub(crate) spherical: Vec<Entity>,
    pub(crate) prismatic: Vec<Entity>,
    pub(crate) distance: Vec<Entity>,
}

impl JointSolverOrder {
    fn clear(&mut self) {
        self.fixed.clear();
        self.revolute.clear();
        self.spherical.clear();
        self.prismatic.clear();
        self.distance.clear();
    }
}

/// The optional affected body set restricts continuous closure to its topology.
/// Ordinary hard sweeps use the entire prepared order.
#[derive(Resource, Default)]
pub(crate) struct PhysicsJointPositionScope(pub(crate) Option<std::collections::HashSet<Entity>>);

trait OrderedJoint: Component + ProjectionJoint + XpbdConstraint<2> + Clone {
    fn drive(&self) -> Option<JointDrive> {
        None
    }
    fn disable_drive(&mut self) {}
    fn solver_entities(order: &JointSolverOrder) -> &[Entity];
    fn local_anchors(&self) -> Option<[avian3d::math::Vector; 2]>;
}
macro_rules! ordered_joint {
    ($joint:ty,$field:ident $(, $methods:item)*) => {
        impl OrderedJoint for $joint {
            $($methods)*
            fn solver_entities(order: &JointSolverOrder) -> &[Entity] {
                &order.$field
            }
            fn local_anchors(&self) -> Option<[avian3d::math::Vector; 2]> {
                Some([self.local_anchor1()?, self.local_anchor2()?])
            }
        }
    };
}
ordered_joint!(FixedJoint, fixed);
ordered_joint!(
    RevoluteJoint,
    revolute,
    fn drive(&self) -> Option<JointDrive> {
        self.motor
            .enabled
            .then_some(JointDrive::Angular(self.motor, self.hinge_axis))
    },
    fn disable_drive(&mut self) {
        self.motor.enabled = false;
    }
);
ordered_joint!(SphericalJoint, spherical);
ordered_joint!(
    PrismaticJoint,
    prismatic,
    fn drive(&self) -> Option<JointDrive> {
        self.motor
            .enabled
            .then_some(JointDrive::Linear(self.motor, self.slider_axis))
    },
    fn disable_drive(&mut self) {
        self.motor.enabled = false;
    }
);
ordered_joint!(DistanceJoint, distance);

/// Replaces Avian's ECS-order-dependent XPBD joint loops with stable-key order.
pub struct DeterministicJointSolverPlugin;

#[derive(Clone, Copy)]
enum JointPass {
    Ordinary,
    Closure,
}

impl Plugin for DeterministicJointSolverPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            !app.is_plugin_added::<XpbdSolverPlugin>(),
            "disable XpbdSolverPlugin when installing DeterministicJointSolverPlugin"
        );
        app.init_resource::<lunco_core::RuntimeFaults>();
        // A failed candidate must never reach native body or contact writeback.
        // This boundary applies to both joints and continuous contacts.
        app.configure_sets(
            SubstepSchedule,
            (
                XpbdSolverSystems::VelocityProjection,
                SubstepSolverSystems::Relax,
            )
                .distributive_run_if(no_solver_fault),
        );
        app.configure_sets(
            PhysicsSchedule,
            (
                SolverSystems::PostSubstep,
                SolverSystems::Restitution,
                SolverSystems::Finalize,
                SolverSystems::StoreContactImpulses,
            )
                .distributive_run_if(no_solver_fault),
        );
        app.configure_sets(
            SubstepSchedule,
            (
                XpbdSolverSystems::SolveConstraints,
                XpbdSolverSystems::SolveUserConstraints,
                XpbdSolverSystems::VelocityProjection,
            )
                .chain()
                .after(IntegrationSystems::Position)
                .before(SubstepSolverSystems::Relax),
        );
        app.configure_sets(
            SubstepSchedule,
            (
                PhysicsJointSolvePass::Fixed,
                PhysicsJointSolvePass::BeforeRevolute,
                PhysicsJointSolvePass::Revolute,
                PhysicsJointSolvePass::Spherical,
                PhysicsJointSolvePass::Prismatic,
                PhysicsJointSolvePass::Distance,
            )
                .chain()
                .after(XpbdSolverSystems::SolveConstraints)
                .before(XpbdSolverSystems::SolveUserConstraints),
        );
        remove_native_damping_systems(app);
        app.register_type::<JointDisabled>();
        app.register_required_components::<FixedJoint, JointProjection>();
        app.register_required_components::<RevoluteJoint, JointProjection>();
        app.register_required_components::<SphericalJoint, JointProjection>();
        app.register_required_components::<PrismaticJoint, JointProjection>();
        app.register_required_components::<DistanceJoint, JointProjection>();
        app.register_required_components::<FixedJoint, FixedJointSolverData>();
        app.register_required_components::<RevoluteJoint, RevoluteJointSolverData>();
        app.register_required_components::<SphericalJoint, SphericalJointSolverData>();
        app.register_required_components::<PrismaticJoint, PrismaticJointSolverData>();
        app.register_required_components::<DistanceJoint, DistanceJointSolverData>();
        app.add_systems(
            PhysicsSchedule,
            (
                prepare_xpbd_joint::<FixedJoint>,
                prepare_xpbd_joint::<RevoluteJoint>,
                prepare_xpbd_joint::<SphericalJoint>,
                prepare_xpbd_joint::<PrismaticJoint>,
                prepare_xpbd_joint::<DistanceJoint>,
            )
                .chain()
                .in_set(SolverSystems::PrepareJoints),
        );
        app.add_systems(
            SubstepSchedule,
            capture_joint_projection_start.in_set(XpbdSolverSystems::SolveConstraints),
        );
        app.add_systems(
            SubstepSchedule,
            project_joint_velocities.in_set(XpbdSolverSystems::VelocityProjection),
        );
        app.init_scene_resource::<PhysicsHolds>()
            .init_scene_resource::<JointSolverOrder>()
            .init_scene_resource::<PhysicsJointPositionScope>()
            .add_systems(
                PhysicsSchedule,
                (
                    prepare_joint_solver_order,
                    prepare_joint_projection::<FixedJoint>,
                    prepare_joint_projection::<RevoluteJoint>,
                    prepare_joint_projection::<SphericalJoint>,
                    prepare_joint_projection::<PrismaticJoint>,
                    prepare_joint_projection::<DistanceJoint>,
                )
                    .chain()
                    .in_set(PhysicsJointOrderPreparation)
                    .in_set(PhysicsStepSystems::Solver)
                    .after(SolverSystems::PrepareJoints)
                    .before(SolverSystems::Substep),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<FixedJoint, FixedJointSolverData>
                    .in_set(PhysicsJointSolvePass::Fixed),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<RevoluteJoint, RevoluteJointSolverData>
                    .in_set(PhysicsJointSolvePass::Revolute),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<SphericalJoint, SphericalJointSolverData>
                    .in_set(PhysicsJointSolvePass::Spherical),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<PrismaticJoint, PrismaticJointSolverData>
                    .in_set(PhysicsJointSolvePass::Prismatic),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<DistanceJoint, DistanceJointSolverData>
                    .in_set(PhysicsJointSolvePass::Distance),
            );
        app.add_systems(
            SubstepSchedule,
            (
                stable_joint_damping::<FixedJoint>,
                stable_joint_damping::<RevoluteJoint>,
                stable_joint_damping::<SphericalJoint>,
                stable_joint_damping::<PrismaticJoint>,
                stable_joint_damping::<DistanceJoint>,
            )
                .chain()
                .in_set(SubstepSolverSystems::Damping),
        );
        app.add_systems(
            PhysicsSchedule,
            (
                prepare_joint_reactions::<FixedJoint>,
                prepare_joint_reactions::<RevoluteJoint>,
                prepare_joint_reactions::<SphericalJoint>,
                prepare_joint_reactions::<PrismaticJoint>,
                prepare_joint_reactions::<DistanceJoint>,
                commit_joint_reactions,
            )
                .chain()
                .after(SolverSystems::Restitution)
                .before(SolverSystems::Finalize),
        );
        app.init_schedule(PhysicsJointPositionSchedule)
            .add_systems(PhysicsJointPositionSchedule, symmetric_joint_position_sweep);
        // The symmetric native sweeps are ordered serially. A closure iteration
        // must execute them directly, without worker dispatch between passes.
        app.get_schedule_mut(PhysicsJointPositionSchedule)
            .expect("position schedule initialized")
            .set_executor(SingleThreadedExecutor::new());
        // Symmetric hard sweeps propagate corrections in both directions across
        // the prepared topology. Motors integrate only in the ordinary pass.
        for index in 0..3 {
            let sweep = JointPositionSweep(index);
            app.configure_sets(
                SubstepSchedule,
                sweep.in_set(XpbdSolverSystems::SolveUserConstraints),
            );
            if index > 0 {
                app.configure_sets(SubstepSchedule, sweep.after(JointPositionSweep(index - 1)));
            }
            app.add_systems(SubstepSchedule, run_position_sweep.in_set(sweep));
        }
    }
}

fn no_solver_fault(faults: Res<lunco_core::RuntimeFaults>) -> bool {
    !faults.active()
}

fn run_position_sweep(world: &mut World) {
    world.run_schedule(PhysicsJointPositionSchedule);
}

fn remove_native_damping_systems(app: &mut App) {
    macro_rules! remove_once {
        ($system:expr) => {{
            let removed = app
                .remove_systems_in_set(
                    SubstepSchedule,
                    $system,
                    ScheduleCleanupPolicy::RemoveSystemsOnly,
                )
                .expect("Avian solver schedule must be installed before application joint damping");
            assert_eq!(
                removed, 1,
                "native passive damping must be installed exactly once"
            );
        }};
    }

    remove_once!(avian3d::dynamics::solver::joint_damping::<FixedJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<RevoluteJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<SphericalJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<PrismaticJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<DistanceJoint>);
}

fn prepare_joint_solver_order(
    fixed: Query<
        (Entity, Option<&PhysicsOrderKey>),
        (
            With<FixedJoint>,
            With<FixedJointSolverData>,
            Without<RigidBody>,
            Without<JointDisabled>,
        ),
    >,
    revolute: Query<
        (Entity, Option<&PhysicsOrderKey>),
        (
            With<RevoluteJoint>,
            With<RevoluteJointSolverData>,
            Without<RigidBody>,
            Without<JointDisabled>,
        ),
    >,
    spherical: Query<
        (Entity, Option<&PhysicsOrderKey>),
        (
            With<SphericalJoint>,
            With<SphericalJointSolverData>,
            Without<RigidBody>,
            Without<JointDisabled>,
        ),
    >,
    prismatic: Query<
        (Entity, Option<&PhysicsOrderKey>),
        (
            With<PrismaticJoint>,
            With<PrismaticJointSolverData>,
            Without<RigidBody>,
            Without<JointDisabled>,
        ),
    >,
    distance: Query<
        (Entity, Option<&PhysicsOrderKey>),
        (
            With<DistanceJoint>,
            With<DistanceJointSolverData>,
            Without<RigidBody>,
            Without<JointDisabled>,
        ),
    >,
    mut order: ResMut<JointSolverOrder>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults
        .as_deref()
        .is_some_and(lunco_core::RuntimeFaults::active)
    {
        return;
    }

    let fixed_order = ordered_physics_entities(fixed.iter(), "FixedJoint");
    let revolute_order = ordered_physics_entities(revolute.iter(), "RevoluteJoint");
    let spherical_order = ordered_physics_entities(spherical.iter(), "SphericalJoint");
    let prismatic_order = ordered_physics_entities(prismatic.iter(), "PrismaticJoint");
    let distance_order = ordered_physics_entities(distance.iter(), "DistanceJoint");
    let invalid = fixed_order
        .as_ref()
        .err()
        .or_else(|| revolute_order.as_ref().err())
        .or_else(|| spherical_order.as_ref().err())
        .or_else(|| prismatic_order.as_ref().err())
        .or_else(|| distance_order.as_ref().err());
    if let Some(invalid) = invalid {
        report_invalid_physics_order(
            Some(&mut holds),
            faults.as_deref_mut(),
            Some(&mut physics_time),
            "physics-joint-order-invalid",
            invalid,
        );
        order.clear();
        return;
    }

    order.fixed = fixed_order.expect("validated fixed joint order");
    order.revolute = revolute_order.expect("validated revolute joint order");
    order.spherical = spherical_order.expect("validated spherical joint order");
    order.prismatic = prismatic_order.expect("validated prismatic joint order");
    order.distance = distance_order.expect("validated distance joint order");
}

fn prepare_joint_projection<C: Component + OrderedJoint>(
    order: Res<JointSolverOrder>,
    bodies: Query<RigidBodyQueryReadOnly, Without<RigidBodyDisabled>>,
    mut joints: Query<(&C, &mut JointProjection), (Without<RigidBody>, Without<JointDisabled>)>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.as_ref().is_some_and(|faults| faults.active()) {
        return;
    }
    for entity in C::solver_entities(&order) {
        let Ok((joint, mut projection)) = joints.get_mut(*entity) else {
            report_stale_joint_order(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
            );
            return;
        };
        let result = joint.geometry().and_then(|geometry| {
            let [first, second] = bodies
                .get_many(joint.entities())
                .map_err(|_| "joint projection endpoint poses are missing or aliased")?;
            projection.prepare(geometry, [&first, &second])
        });
        if let Err(detail) = result {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-projection-invalid",
                detail,
            );
            return;
        }
    }
}

fn capture_joint_projection_start(
    mut bodies: Query<
        (
            &SolverBody,
            &mut PreSolveDeltaPosition,
            &mut PreSolveDeltaRotation,
        ),
        Without<RigidBodyDisabled>,
    >,
) {
    for (body, mut position, mut rotation) in &mut bodies {
        position.0 = body.delta_position;
        rotation.0 = body.delta_rotation;
    }
}

fn project_joint_velocities(
    mut bodies: Query<
        (
            Entity,
            &mut SolverBody,
            &PreSolveDeltaPosition,
            &PreSolveDeltaRotation,
        ),
        (Without<RigidBodyDisabled>, Without<Sleeping>),
    >,
    time: Res<Time<Substeps>>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.as_ref().is_some_and(|faults| faults.active()) {
        return;
    }
    let dt = time.delta_secs_f64();
    if dt == 0.0 {
        return;
    }
    for (entity, mut body, position, rotation) in &mut bodies {
        let result = crate::joint::rotation_displacement(rotation.0.0, body.delta_rotation.0)
            .and_then(|displacement| {
                let linear = body.linear_velocity + (body.delta_position - position.0) / dt;
                let angular = body.angular_velocity + displacement / dt;
                if !linear.is_finite() || !angular.is_finite() {
                    return Err("joint velocity projection is not representable");
                }
                Ok((linear, angular))
            });
        match result {
            Ok((linear, angular)) => {
                body.linear_velocity = linear;
                body.angular_velocity = angular;
            }
            Err(detail) => {
                report_joint_solver_fault(
                    &mut holds,
                    faults.as_deref_mut(),
                    &mut physics_time,
                    entity,
                    "joint velocity projection",
                    "physics-joint-velocity-invalid",
                    detail,
                );
                return;
            }
        }
    }
}

fn stable_solve_xpbd_joint<C, D>(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut joints: Query<(&mut C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
    mut projections: Query<&mut JointProjection>,
    order: Res<JointSolverOrder>,
    time: Res<Time>,
    scope: Res<PhysicsJointPositionScope>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) where
    C: Component<Mutability = Mutable>
        + EntityConstraint<2>
        + OrderedJoint
        + XpbdConstraint<2, SolverData = D>,
    D: Component<Mutability = Mutable>,
{
    solve_ordered_joint::<C, D, false>(
        &bodies,
        &mut joints,
        &mut projections,
        &order,
        time.delta_secs_f64(),
        JointPass::Ordinary,
        &scope,
        &mut holds,
        faults.as_deref_mut(),
        &mut physics_time,
    );
}

/// One native query acquisition and one serial system boundary for the entire
/// symmetric sweep. Constraint type order and per-type entity order are the
/// same in both directions; ordinary passes retain their extension-point sets.
fn symmetric_joint_position_sweep(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut fixed: Query<
        (&mut FixedJoint, &mut FixedJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    mut revolute: Query<
        (&mut RevoluteJoint, &mut RevoluteJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    mut spherical: Query<
        (&mut SphericalJoint, &mut SphericalJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    mut prismatic: Query<
        (&mut PrismaticJoint, &mut PrismaticJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    mut distance: Query<
        (&mut DistanceJoint, &mut DistanceJointSolverData),
        (Without<RigidBody>, Without<JointDisabled>),
    >,
    mut projections: Query<&mut JointProjection>,
    order: Res<JointSolverOrder>,
    time: Res<Time>,
    scope: Res<PhysicsJointPositionScope>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    macro_rules! pass {
        ($kind:ty, $data:ty, $query:ident, $reverse:literal) => {
            if !solve_ordered_joint::<$kind, $data, $reverse>(
                &bodies,
                &mut $query,
                &mut projections,
                &order,
                time.delta_secs_f64(),
                JointPass::Closure,
                &scope,
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
            ) {
                return;
            }
        };
    }
    pass!(FixedJoint, FixedJointSolverData, fixed, false);
    pass!(RevoluteJoint, RevoluteJointSolverData, revolute, false);
    pass!(SphericalJoint, SphericalJointSolverData, spherical, false);
    pass!(PrismaticJoint, PrismaticJointSolverData, prismatic, false);
    pass!(DistanceJoint, DistanceJointSolverData, distance, false);
    pass!(DistanceJoint, DistanceJointSolverData, distance, true);
    pass!(PrismaticJoint, PrismaticJointSolverData, prismatic, true);
    pass!(SphericalJoint, SphericalJointSolverData, spherical, true);
    pass!(RevoluteJoint, RevoluteJointSolverData, revolute, true);
    pass!(FixedJoint, FixedJointSolverData, fixed, true);
}

fn solve_ordered_joint<C, D, const REVERSE: bool>(
    bodies: &Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    joints: &mut Query<(&mut C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
    projections: &mut Query<&mut JointProjection>,
    order: &JointSolverOrder,
    delta_secs: f64,
    pass: JointPass,
    scope: &PhysicsJointPositionScope,
    holds: &mut PhysicsHolds,
    mut faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: &mut Time<Physics>,
) -> bool
where
    C: Component<Mutability = Mutable>
        + EntityConstraint<2>
        + OrderedJoint
        + XpbdConstraint<2, SolverData = D>,
    D: Component<Mutability = Mutable>,
{
    if faults
        .as_deref()
        .is_some_and(lunco_core::RuntimeFaults::active)
    {
        return false;
    }
    let ordered = C::solver_entities(&order);
    let mut dummy_body1 = SolverBody::default();
    let mut dummy_body2 = SolverBody::default();

    for index in 0..ordered.len() {
        let entity = &ordered[if REVERSE {
            ordered.len() - 1 - index
        } else {
            index
        }];
        let Ok((joint, mut solver_data)) = joints.get_mut(*entity) else {
            report_stale_joint_order(
                holds,
                faults.as_deref_mut(),
                physics_time,
                *entity,
                std::any::type_name::<C>(),
            );
            return false;
        };
        let [entity1, entity2] = joint.entities();
        if entity1 == entity2 {
            report_joint_solver_fault(
                holds,
                faults.as_deref_mut(),
                physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-endpoints-invalid",
                "joint endpoints resolve to the same body",
            );
            return false;
        }
        if matches!(pass, JointPass::Closure)
            && scope
                .0
                .as_ref()
                .is_some_and(|bodies| !bodies.contains(&entity1) && !bodies.contains(&entity2))
        {
            continue;
        }
        let (mut body1, mut inertia1) = (&mut dummy_body1, &SolverBodyInertia::DUMMY);
        let (mut body2, mut inertia2) = (&mut dummy_body2, &SolverBodyInertia::DUMMY);

        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(entity1) } {
            body1 = body.into_inner();
            inertia1 = inertia;
        }
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(entity2) } {
            body2 = body.into_inner();
            inertia2 = inertia;
        }

        match (inertia1.dominance() - inertia2.dominance()).cmp(&0) {
            std::cmp::Ordering::Greater => inertia1 = &SolverBodyInertia::DUMMY,
            std::cmp::Ordering::Less => inertia2 = &SolverBodyInertia::DUMMY,
            std::cmp::Ordering::Equal => {}
        }

        if matches!(pass, JointPass::Closure) {
            let Ok(mut projection) = projections.get_mut(*entity) else {
                report_stale_joint_order(
                    holds,
                    faults.as_deref_mut(),
                    physics_time,
                    *entity,
                    std::any::type_name::<C>(),
                );
                return false;
            };
            if let Err(detail) = projection.solve([body1, body2], [inertia1, inertia2]) {
                report_joint_solver_fault(
                    holds,
                    faults.as_deref_mut(),
                    physics_time,
                    *entity,
                    std::any::type_name::<C>(),
                    "physics-joint-projection-invalid",
                    detail,
                );
                return false;
            }
        } else {
            let Ok(mut projection) = projections.get_mut(*entity) else {
                report_stale_joint_order(
                    holds,
                    faults.as_deref_mut(),
                    physics_time,
                    *entity,
                    std::any::type_name::<C>(),
                );
                return false;
            };
            if let Some(drive) = joint.drive()
                && let Err(detail) = projection.integrate_drive(
                    drive,
                    [&mut *body1, &mut *body2],
                    [inertia1, inertia2],
                    delta_secs,
                )
            {
                report_joint_solver_fault(
                    holds,
                    faults.as_deref_mut(),
                    physics_time,
                    *entity,
                    std::any::type_name::<C>(),
                    "physics-joint-drive-invalid",
                    detail,
                );
                return false;
            }
            let mut native = (*joint).clone();
            native.disable_drive();
            native.solve(
                [body1, body2],
                [inertia1, inertia2],
                &mut solver_data,
                delta_secs,
            );
        }
    }
    true
}

fn stable_joint_damping<C: Component + EntityConstraint<2> + OrderedJoint>(
    order: Res<JointSolverOrder>,
    joints: Query<(&C, Option<&JointDamping>)>,
    bodies: Query<(&mut SolverBody, &SolverBodyInertia)>,
    poses: Query<RigidBodyQueryReadOnly, Without<RigidBodyDisabled>>,
    time: Res<Time<Substeps>>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.as_ref().is_some_and(|f| f.active()) {
        return;
    }
    for entity in C::solver_entities(&order) {
        let Ok((joint, damping)) = joints.get(*entity) else {
            report_stale_joint_order(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
            );
            return;
        };
        let Some(damping) = damping else {
            continue;
        };
        let [a, b] = joint.entities();
        if a == b {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-endpoints-invalid",
                "joint endpoints resolve to the same body",
            );
            return;
        }
        let Some(anchors) = joint.local_anchors() else {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-damping-frame-invalid",
                "joint damping requires resolved local anchors",
            );
            return;
        };
        let Ok([pose1, pose2]) = poses.get_many([a, b]) else {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-damping-frame-invalid",
                "joint damping endpoint pose is unavailable",
            );
            return;
        };
        let mut dummy1 = SolverBody::DUMMY;
        let mut dummy2 = SolverBody::DUMMY;
        let (mut body1, mut inertia1) = (&mut dummy1, &SolverBodyInertia::DUMMY);
        let (mut body2, mut inertia2) = (&mut dummy2, &SolverBodyInertia::DUMMY);
        // Distinct endpoints were validated before taking disjoint mutable references.
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(a) } {
            body1 = body.into_inner();
            inertia1 = inertia;
        }
        if let Ok((body, inertia)) = unsafe { bodies.get_unchecked(b) } {
            body2 = body.into_inner();
            inertia2 = inertia;
        }
        let anchor1 =
            body1.delta_rotation * (pose1.rotation * (anchors[0] - pose1.center_of_mass.0));
        let anchor2 =
            body2.delta_rotation * (pose2.rotation * (anchors[1] - pose2.center_of_mass.0));
        let centers = (pose2.position.0 - pose1.position.0)
            + (pose2.rotation * pose2.center_of_mass.0 - pose1.rotation * pose1.center_of_mass.0)
            + (body2.delta_position - body1.delta_position);
        // Both impulses act at the midpoint of the current joint anchors, so
        // their orbital moments cancel even while position closure has error.
        let levers = [
            (anchor1 + anchor2 + centers) * 0.5,
            (anchor1 + anchor2 - centers) * 0.5,
        ];
        if let Err(detail) = crate::joint::integrate_joint_damping(
            [body1, body2],
            [inertia1, inertia2],
            levers,
            *damping,
            time.delta_secs_f64(),
        ) {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-damping-invalid",
                detail,
            );
            return;
        }
    }
}

fn prepare_joint_reactions<C: Component + OrderedJoint + XpbdConstraint<2>>(
    order: Res<JointSolverOrder>,
    mut joints: Query<(
        &C,
        &C::SolverData,
        &mut JointProjection,
        Option<&JointForces>,
        Has<JointDisabled>,
    )>,
    bodies: Query<(&RigidBody, Has<RigidBodyDisabled>, Has<Sleeping>)>,
    mut physics: ResMut<Time<Physics>>,
    substeps: Res<Time<Substeps>>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) where
    C::SolverData: Component,
{
    // Every candidate belongs to this completed step. Disabled joints have no
    // active prepared-order entry, but still clear their reaction on commit.
    for (_, _, mut projection, forces, disabled) in &mut joints {
        projection.pending_reaction = None;
        if disabled && forces.is_some() {
            projection.pending_reaction = Some(JointForces::new());
        }
    }
    if faults.as_ref().is_some_and(|faults| faults.active()) {
        return;
    }
    let duration = physics.delta_secs_f64();
    if duration == 0.0 {
        return;
    }
    for entity in C::solver_entities(&order) {
        let Ok((joint, data, mut projection, forces, _)) = joints.get_mut(*entity) else {
            report_stale_joint_order(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics,
                *entity,
                std::any::type_name::<C>(),
            );
            return;
        };
        if forces.is_none() {
            continue;
        }
        let Ok(endpoints) = bodies.get_many(joint.entities()) else {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-reaction-invalid",
                "joint reaction endpoints are missing or aliased",
            );
            return;
        };
        let disabled = endpoints.iter().any(|(_, disabled, _)| *disabled);
        if disabled {
            projection.pending_reaction = Some(JointForces::new());
            continue;
        }
        let inactive = endpoints
            .iter()
            .all(|(body, _, sleeping)| !body.is_dynamic() || *sleeping);
        if inactive {
            continue;
        }
        let substep = substeps.delta_secs_f64();
        let denominator = duration * substep;
        if !denominator.is_finite() || denominator <= 0.0 {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-reaction-invalid",
                "completed joint reaction requires representable positive physics and substep durations",
            );
            return;
        }
        let force = (data.total_position_lagrange() + projection.position_lagrange) / denominator;
        let torque = (data.total_rotation_lagrange() + projection.rotation_lagrange) / denominator;
        let motor = projection.motor_impulse / duration;
        if !force.is_finite() || !torque.is_finite() || !motor.is_finite() {
            report_joint_solver_fault(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics,
                *entity,
                std::any::type_name::<C>(),
                "physics-joint-reaction-invalid",
                "completed joint reaction is not finite",
            );
            return;
        }
        let mut forces = JointForces::new();
        forces.set_force(force);
        forces.set_torque(torque);
        forces.set_motor_force(motor);
        projection.pending_reaction = Some(forces);
    }
}

fn commit_joint_reactions(
    mut joints: Query<(&mut JointProjection, &mut JointForces)>,
    faults: Res<lunco_core::RuntimeFaults>,
) {
    if faults.active() {
        return;
    }
    for (mut projection, mut forces) in &mut joints {
        if let Some(completed) = projection.pending_reaction.take() {
            *forces = completed;
        }
    }
}

fn report_stale_joint_order(
    holds: &mut PhysicsHolds,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: &mut Time<Physics>,
    entity: Entity,
    joint_type: &'static str,
) {
    report_joint_solver_fault(
        holds,
        faults,
        physics_time,
        entity,
        joint_type,
        "physics-joint-order-stale",
        "solver topology changed after deterministic order preparation",
    );
}
fn report_joint_solver_fault(
    holds: &mut PhysicsHolds,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: &mut Time<Physics>,
    entity: Entity,
    joint_type: &'static str,
    code: &'static str,
    detail: &str,
) {
    holds.set(PhysicsHolds::SAFETY_FAILURE, true);
    physics_time.pause();
    physics_time.advance_by(Duration::ZERO);
    let has_fault_resource = faults.is_some();
    let won = faults.is_some_and(|faults| faults.raise(code, Some(entity), joint_type, detail));
    if won || !has_fault_resource {
        error!("[physics] stopping the step: {detail}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    /// Exercises the native schedule/finalization seam, including a failure
    /// after one valid reaction has already been staged for the same step.
    #[test]
    fn reaction_failure_refuses_the_entire_completed_step_writeback() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            PhysicsPlugins::default()
                .build()
                .disable::<XpbdSolverPlugin>(),
            DeterministicJointSolverPlugin,
        ));
        app.add_message::<AssetEvent<Mesh>>();
        app.init_resource::<Assets<Mesh>>();
        app.insert_resource(Gravity::ZERO);
        app.finish();
        app.cleanup();
        let anchor = app
            .world_mut()
            .spawn((
                RigidBody::Static,
                Position(DVec3::ZERO),
                PhysicsOrderKey("anchor".into()),
            ))
            .id();
        let mut bodies = Vec::new();
        let mut joints = Vec::new();
        for (index, label) in ["a", "b"].into_iter().enumerate() {
            let offset = DVec3::X * (index + 1) as f64;
            let body = app
                .world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Collider::sphere(0.1),
                    Position(offset),
                    LinearVelocity(DVec3::Y * 10.0),
                    PhysicsOrderKey(format!("body-{label}")),
                ))
                .id();
            let joint = app
                .world_mut()
                .spawn((
                    FixedJoint::new(anchor, body).with_local_anchor1(offset),
                    JointForces::new(),
                    PhysicsOrderKey(format!("joint-{label}")),
                ))
                .id();
            bodies.push(body);
            joints.push(joint);
        }
        app.update();
        let step = Duration::from_secs_f64(1.0 / 60.0);
        app.world_mut()
            .resource_mut::<Time<Physics>>()
            .advance_by(step);
        app.world_mut().run_schedule(PhysicsSchedule);
        assert!(!app.world().resource::<lunco_core::RuntimeFaults>().active());
        let completed: Vec<_> = joints
            .iter()
            .map(|entity| *app.world().get::<JointForces>(*entity).unwrap())
            .collect();
        assert!(completed.iter().all(|forces| forces.force().length() > 0.0));

        let invalid = joints[1];
        app.add_systems(
            PhysicsSchedule,
            (move |mut projections: Query<&mut JointProjection>| {
                projections.get_mut(invalid).unwrap().position_lagrange = DVec3::NAN;
            })
            .in_set(SolverSystems::PostSubstep),
        );
        for body in &bodies {
            app.world_mut().get_mut::<LinearVelocity>(*body).unwrap().0 = DVec3::Y * 20.0;
        }
        let positions: Vec<_> = bodies
            .iter()
            .map(|entity| app.world().get::<Position>(*entity).unwrap().0)
            .collect();
        app.world_mut()
            .resource_mut::<Time<Physics>>()
            .advance_by(step);
        app.world_mut().run_schedule(PhysicsSchedule);
        let fault = app
            .world()
            .resource::<lunco_core::RuntimeFaults>()
            .first
            .as_ref()
            .unwrap();
        assert_eq!(fault.kind, "physics-joint-reaction-invalid");
        assert_eq!(fault.entity, Some(invalid));
        assert!(app.world().resource::<PhysicsHolds>().is_held());
        assert!(app.world().resource::<Time<Physics>>().is_paused());
        assert_eq!(
            app.world().resource::<Time<Physics>>().delta(),
            Duration::ZERO
        );
        let pending = app
            .world()
            .get::<JointProjection>(joints[0])
            .unwrap()
            .pending_reaction
            .unwrap();
        assert_ne!(
            pending, completed[0],
            "a changed valid reaction was staged before the later fault"
        );
        for (index, body) in bodies.iter().enumerate() {
            assert_eq!(
                app.world().get::<Position>(*body).unwrap().0,
                positions[index]
            );
            assert_eq!(
                app.world().get::<LinearVelocity>(*body).unwrap().0,
                DVec3::Y * 20.0
            );
            assert_eq!(
                *app.world().get::<JointForces>(joints[index]).unwrap(),
                completed[index]
            );
        }
    }
}

impl lunco_core::SceneResource for JointSolverOrder {}

impl lunco_core::SceneResource for PhysicsJointPositionScope {}
