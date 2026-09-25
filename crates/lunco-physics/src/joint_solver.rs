//! Stable ordering for XPBD constraints that mutate shared solver bodies.

use std::time::Duration;

use crate::order::{ordered_physics_entities, report_invalid_physics_order};
use crate::{PhysicsHolds, PhysicsOrderKey};
use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    SolverConfig,
    schedule::{SolverSystems, SubstepSolverSystems},
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{XpbdConstraint, XpbdSolverSystems, joints::*},
};
use avian3d::prelude::*;
use bevy::ecs::component::Mutable;
use bevy::ecs::schedule::ScheduleCleanupPolicy;
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

/// Runs after Avian's general warm-start pass and before XPBD constraints.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PhysicsJointWarmStart;

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

trait OrderedJoint: Component {
    fn solver_entities(order: &JointSolverOrder) -> &[Entity];
}

macro_rules! ordered_joint {
    ($joint:ty, $field:ident) => {
        impl OrderedJoint for $joint {
            fn solver_entities(order: &JointSolverOrder) -> &[Entity] {
                &order.$field
            }
        }
    };
}

ordered_joint!(FixedJoint, fixed);
ordered_joint!(RevoluteJoint, revolute);
ordered_joint!(SphericalJoint, spherical);
ordered_joint!(PrismaticJoint, prismatic);
ordered_joint!(DistanceJoint, distance);

/// Replaces Avian's ECS-order-dependent XPBD joint loops with stable-key order.
pub struct DeterministicJointSolverPlugin;

impl Plugin for DeterministicJointSolverPlugin {
    fn build(&self, app: &mut App) {
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
        )
        .configure_sets(
            SubstepSchedule,
            PhysicsJointWarmStart
                .after(SubstepSolverSystems::WarmStart)
                .before(SubstepSolverSystems::SolveConstraints),
        );
        remove_native_joint_systems(app);
        app.init_resource::<PhysicsHolds>()
            .init_resource::<JointSolverOrder>()
            .add_systems(
                PhysicsSchedule,
                prepare_joint_solver_order
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
            )
            .add_systems(
                SubstepSchedule,
                (
                    stable_warm_start_xpbd_motors::<RevoluteJoint, RevoluteJointSolverData>,
                    stable_warm_start_xpbd_motors::<PrismaticJoint, PrismaticJointSolverData>,
                )
                    .chain()
                    .in_set(PhysicsJointWarmStart),
            );
    }
}

fn remove_native_joint_systems(app: &mut App) {
    macro_rules! remove_once {
        ($system:expr) => {{
            let removed = app
                .remove_systems_in_set(
                    SubstepSchedule,
                    $system,
                    ScheduleCleanupPolicy::RemoveSystemsOnly,
                )
                .expect(
                    "Avian XPBD solver schedule must be installed before stable joint ordering",
                );
            assert_eq!(
                removed, 1,
                "Avian XPBD joint system must be installed exactly once"
            );
        }};
    }

    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<FixedJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<RevoluteJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<SphericalJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<PrismaticJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<DistanceJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::warm_start_xpbd_motors::<RevoluteJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::warm_start_xpbd_motors::<PrismaticJoint>);
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

fn stable_solve_xpbd_joint<C, D>(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut joints: Query<(&mut C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
    order: Res<JointSolverOrder>,
    time: Res<Time>,
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
    let ordered = C::solver_entities(&order);
    let delta_secs = time.delta_secs_f64();
    let mut dummy_body1 = SolverBody::default();
    let mut dummy_body2 = SolverBody::default();

    for entity in ordered {
        let Ok((mut joint, mut solver_data)) = joints.get_mut(*entity) else {
            report_stale_joint_order(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
            );
            return;
        };
        let [entity1, entity2] = joint.entities();
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

        joint.solve(
            [body1, body2],
            [inertia1, inertia2],
            &mut solver_data,
            delta_secs,
        );
    }
}

fn stable_warm_start_xpbd_motors<C, D>(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut joints: Query<(&C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
    order: Res<JointSolverOrder>,
    time: Res<Time>,
    solver_config: Res<SolverConfig>,
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
    let ordered = C::solver_entities(&order);
    let delta_secs = time.delta_secs_f64();
    let mut dummy_body1 = SolverBody::default();
    let mut dummy_body2 = SolverBody::default();

    for entity in ordered {
        let Ok((joint, mut solver_data)) = joints.get_mut(*entity) else {
            report_stale_joint_order(
                &mut holds,
                faults.as_deref_mut(),
                &mut physics_time,
                *entity,
                std::any::type_name::<C>(),
            );
            return;
        };
        let [entity1, entity2] = joint.entities();
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

        joint.warm_start_motors(
            [body1, body2],
            [inertia1, inertia2],
            &mut solver_data,
            delta_secs,
            solver_config.warm_start_coefficient,
        );
    }
}

fn report_stale_joint_order(
    holds: &mut PhysicsHolds,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: &mut Time<Physics>,
    entity: Entity,
    joint_type: &'static str,
) {
    holds.set(PhysicsHolds::SAFETY_FAILURE, true);
    physics_time.pause();
    physics_time.advance_by(Duration::ZERO);
    let detail = "solver topology changed after deterministic order preparation";
    let has_fault_resource = faults.is_some();
    let won = faults.is_some_and(|faults| {
        faults.raise(
            "physics-joint-order-stale",
            Some(entity),
            joint_type,
            detail,
        )
    });
    if won || !has_fault_resource {
        error!("[physics] stopping the step because a prepared joint order became stale");
    }
}
