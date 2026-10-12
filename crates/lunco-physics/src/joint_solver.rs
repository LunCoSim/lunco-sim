//! Stable ordering for XPBD constraints that mutate shared solver bodies.

use std::time::Duration;

use crate::order::{ordered_physics_entities, report_invalid_physics_order};
use crate::{PhysicsHolds, PhysicsOrderKey};
use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::{
    schedule::{SolverSystems, SubstepSolverSystems},
    solver_body::{SolverBody, SolverBodyInertia},
    xpbd::{HardJointConstraint, XpbdConstraint, XpbdSolverSystems, joints::*},
};
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

trait OrderedJoint: Component {
    fn solver_entities(order: &JointSolverOrder) -> &[Entity];
}
macro_rules! ordered_joint {
    ($joint:ty,$field:ident) => {
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
        );
        remove_native_joint_systems(app);
        app.register_type::<JointDisabled>();
        app.init_resource::<PhysicsHolds>()
            .init_resource::<JointSolverOrder>()
            .init_resource::<PhysicsJointPositionScope>()
            .add_systems(
                PhysicsSchedule,
                prepare_joint_solver_order
                    .in_set(PhysicsJointOrderPreparation)
                    .in_set(PhysicsStepSystems::Solver)
                    .after(SolverSystems::PrepareJoints)
                    .before(SolverSystems::Substep),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<FixedJoint, FixedJointSolverData, false, false>
                    .in_set(PhysicsJointSolvePass::Fixed),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<RevoluteJoint, RevoluteJointSolverData, false, false>
                    .in_set(PhysicsJointSolvePass::Revolute),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<SphericalJoint, SphericalJointSolverData, false, false>
                    .in_set(PhysicsJointSolvePass::Spherical),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<PrismaticJoint, PrismaticJointSolverData, false, false>
                    .in_set(PhysicsJointSolvePass::Prismatic),
            )
            .add_systems(
                SubstepSchedule,
                stable_solve_xpbd_joint::<DistanceJoint, DistanceJointSolverData, false, false>
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

fn run_position_sweep(world: &mut World) {
    world.run_schedule(PhysicsJointPositionSchedule);
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

    remove_once!(avian3d::dynamics::solver::joint_damping::<FixedJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<RevoluteJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<SphericalJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<PrismaticJoint>);
    remove_once!(avian3d::dynamics::solver::joint_damping::<DistanceJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<FixedJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<RevoluteJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<SphericalJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<PrismaticJoint>);
    remove_once!(avian3d::dynamics::solver::xpbd::solve_xpbd_joint::<DistanceJoint>);
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

fn stable_solve_xpbd_joint<C, D, const POSITION_ONLY: bool, const REVERSE: bool>(
    bodies: Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    mut joints: Query<(&mut C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
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
        + HardJointConstraint
        + XpbdConstraint<2, SolverData = D>,
    D: Component<Mutability = Mutable>,
{
    solve_ordered_joint::<C, D, POSITION_ONLY, REVERSE>(
        &bodies,
        &mut joints,
        &order,
        time.delta_secs_f64(),
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
    order: Res<JointSolverOrder>,
    time: Res<Time>,
    scope: Res<PhysicsJointPositionScope>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    macro_rules! pass {
        ($kind:ty, $data:ty, $query:ident, $reverse:literal) => {
            if !solve_ordered_joint::<$kind, $data, true, $reverse>(
                &bodies,
                &mut $query,
                &order,
                time.delta_secs_f64(),
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

fn solve_ordered_joint<C, D, const POSITION_ONLY: bool, const REVERSE: bool>(
    bodies: &Query<(&mut SolverBody, &SolverBodyInertia), Without<RigidBodyDisabled>>,
    joints: &mut Query<(&mut C, &mut D), (Without<RigidBody>, Without<JointDisabled>)>,
    order: &JointSolverOrder,
    delta_secs: f64,
    scope: &PhysicsJointPositionScope,
    holds: &mut PhysicsHolds,
    mut faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: &mut Time<Physics>,
) -> bool
where
    C: Component<Mutability = Mutable>
        + EntityConstraint<2>
        + OrderedJoint
        + HardJointConstraint
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
        let Ok((mut joint, mut solver_data)) = joints.get_mut(*entity) else {
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
        if POSITION_ONLY
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

        if POSITION_ONLY {
            joint.solve_hard_constraints(
                [body1, body2],
                [inertia1, inertia2],
                &mut solver_data,
                delta_secs,
            );
        } else {
            joint.solve(
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
        avian3d::dynamics::solver::apply_joint_damping(
            [body1, body2],
            [inertia1, inertia2],
            *damping,
            time.delta_secs_f64(),
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
