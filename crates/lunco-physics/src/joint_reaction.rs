//! Retain the last completed native joint measurement while its island sleeps.
//!
//! Avian clears its per-step accumulator before preparing active solver bodies.
//! A sleeping island has no new solve. This owner republishes its exact previous
//! native report through `JointForces`; consumers keep one measurement path.

use avian3d::dynamics::joints::EntityConstraint;
use avian3d::dynamics::solver::solver_body::SolverBodyIndex;
use avian3d::prelude::*;
use bevy::prelude::*;

#[derive(Component)]
struct CompletedJointReaction<C: Component> {
    definition: Option<C>,
    last: Option<JointForces>,
    solved: bool,
    retain: bool,
}

impl<C: Component> Default for CompletedJointReaction<C> {
    fn default() -> Self {
        Self {
            definition: None,
            last: None,
            solved: false,
            retain: false,
        }
    }
}

fn install_joint<C: Component + EntityConstraint<2> + Clone + PartialEq>(app: &mut App) {
    app.register_required_components::<C, CompletedJointReaction<C>>()
        .add_observer(invalidate_removed_joint::<C>)
        .add_observer(invalidate_disabled_joint::<C>)
        .add_observer(invalidate_disabled_body::<C>)
        .add_systems(
            PhysicsSchedule,
            prepare_measurement::<C>.in_set(SolverSystems::PreSubstep),
        );
}

pub(crate) fn install(app: &mut App) {
    install_joint::<FixedJoint>(app);
    install_joint::<PrismaticJoint>(app);
    install_joint::<RevoluteJoint>(app);
    install_joint::<SphericalJoint>(app);
    install_joint::<DistanceJoint>(app);
    app.add_systems(
        PhysicsSchedule,
        (
            publish_measurement::<FixedJoint>,
            publish_measurement::<PrismaticJoint>,
            publish_measurement::<RevoluteJoint>,
            publish_measurement::<SphericalJoint>,
            publish_measurement::<DistanceJoint>,
        )
            .chain()
            .after(PhysicsStepSystems::Solver)
            .before(PhysicsStepSystems::Sleeping),
    );
}

fn clear_sample<C: Component>(
    sample: &mut CompletedJointReaction<C>,
    forces: Option<Mut<JointForces>>,
) {
    sample.last = None;
    sample.solved = false;
    sample.retain = false;
    if let Some(mut forces) = forces {
        *forces = JointForces::default();
    }
}

fn invalidate_removed_joint<C: Component>(
    event: On<Remove<C>>,
    mut samples: Query<(&mut CompletedJointReaction<C>, Option<&mut JointForces>)>,
) {
    if let Ok((mut sample, forces)) = samples.get_mut(event.entity) {
        sample.definition = None;
        clear_sample(&mut sample, forces);
    }
}

fn invalidate_disabled_joint<C: Component>(
    event: On<Add<JointDisabled>>,
    mut samples: Query<(&mut CompletedJointReaction<C>, Option<&mut JointForces>)>,
) {
    if let Ok((mut sample, forces)) = samples.get_mut(event.entity) {
        clear_sample(&mut sample, forces);
    }
}

fn invalidate_disabled_body<C: Component + EntityConstraint<2>>(
    event: On<Add<RigidBodyDisabled>>,
    mut joints: Query<(&C, &mut CompletedJointReaction<C>, Option<&mut JointForces>)>,
) {
    for (joint, mut sample, forces) in &mut joints {
        if joint.entities().contains(&event.entity) {
            clear_sample(&mut sample, forces);
        }
    }
}

type EndpointState = (
    &'static RigidBody,
    Has<RigidBodyDisabled>,
    Has<Sleeping>,
    Option<&'static SolverBodyIndex>,
);

fn prepare_measurement<C: Component + EntityConstraint<2> + Clone + PartialEq>(
    mut joints: Query<(
        Ref<C>,
        Ref<JointForces>,
        Has<JointDisabled>,
        &mut CompletedJointReaction<C>,
    )>,
    bodies: Query<EndpointState>,
) {
    for (joint, forces, disabled, mut sample) in &mut joints {
        // Re-insertion is a new definition lease even when its fields compare
        // equal. A removed definition cannot publish, and its replacement never
        // inherits a prior definition's completed report.
        if joint.is_added() || forces.is_added() || sample.definition.as_ref() != Some(&*joint) {
            sample.last = None;
            sample.definition = Some((*joint).clone());
        }
        let mut enabled = !disabled;
        let mut active_dynamic = false;
        let mut sleeping_dynamic = false;
        for endpoint in joint.entities() {
            let Ok((body, disabled, sleeping, solver_index)) = bodies.get(endpoint) else {
                enabled = false;
                break;
            };
            enabled &= !disabled;
            active_dynamic |= body.is_dynamic()
                && !sleeping
                && solver_index.is_some_and(SolverBodyIndex::is_valid);
            sleeping_dynamic |= body.is_dynamic() && sleeping;
        }
        sample.solved = enabled && active_dynamic;
        sample.retain = enabled && !active_dynamic && sleeping_dynamic;
        if !enabled || (!sample.solved && !sample.retain) {
            sample.last = None;
        }
    }
}

fn publish_measurement<C: Component + EntityConstraint<2>>(
    mut joints: Query<(
        &C,
        Has<JointDisabled>,
        &mut JointForces,
        &mut CompletedJointReaction<C>,
    )>,
    bodies: Query<(), (With<RigidBody>, Without<RigidBodyDisabled>)>,
) {
    for (joint, disabled, mut forces, mut sample) in &mut joints {
        let enabled = !disabled
            && joint
                .entities()
                .into_iter()
                .all(|body| bodies.contains(body));
        if !enabled {
            sample.last = None;
            *forces = JointForces::default();
        } else if sample.solved {
            // Native writeback and passive damping have completed, while the
            // sleep-transition decision has not yet changed body participation.
            sample.last = Some(*forces);
        } else if sample.retain {
            if let Some(last) = sample.last {
                *forces = last;
            }
        } else {
            *forces = JointForces::default();
        }
    }
}
