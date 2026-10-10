//! Continuous collision advancement of articulated solver trajectories.

use std::collections::HashMap;

use avian3d::dynamics::solver::{schedule::SolverSystems, solver_body::SolverBody};
use avian3d::parry::math::Pose;
use avian3d::parry::query::{
    NonlinearRigidMotion, ShapeCastOptions, cast_shapes, cast_shapes_nonlinear,
};
use avian3d::prelude::*;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;

use crate::{
    PhysicsHolds, PhysicsJointLink, PhysicsJointPending, PhysicsJointTopologyPending,
    dynamic_joint_islands,
};

/// Replaces body-only CCD with one advancement fraction per dynamic joint island.
/// Install with Avian's `CcdPlugin` disabled: there is one CCD pose writer.
pub struct ArticulatedCcdPlugin;

impl Plugin for ArticulatedCcdPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PhysicsHolds>()
            .init_resource::<lunco_core::RuntimeFaults>()
            .add_systems(
                PhysicsSchedule,
                advance_ccd_islands
                    .after(SolverSystems::PostSubstep)
                    .before(SolverSystems::Restitution)
                    .in_set(PhysicsStepSystems::Solver),
            );
    }
}

/// The sweep follows the solved COM translation and rotation, including joint
/// corrections. Collider-local COM allows the same query for child colliders.
fn solved_motion(
    collider_pos: &Position,
    collider_rot: &Rotation,
    body_pos: &Position,
    body_rot: &Rotation,
    com: &ComputedCenterOfMass,
    solver: Option<&SolverBody>,
) -> NonlinearRigidMotion {
    let center_world = body_pos.0 + body_rot.0 * com.0;
    let center_local = collider_rot.0.inverse() * (center_world - collider_pos.0);
    NonlinearRigidMotion::new(
        Pose::from_parts(collider_pos.0, collider_rot.0),
        center_local,
        solver.map_or(DVec3::ZERO, |body| body.delta_position),
        solver.map_or(DVec3::ZERO, |body| body.delta_rotation.0.to_scaled_axis()),
    )
}

#[allow(clippy::type_complexity)]
fn advance_ccd_islands(
    mut bodies: Query<
        (
            Entity,
            &RigidBody,
            &Position,
            &Rotation,
            &ComputedCenterOfMass,
            Option<&SweptCcd>,
            Option<&mut SolverBody>,
        ),
        Without<RigidBodyDisabled>,
    >,
    colliders: Query<(&Collider, &ColliderOf, &Position, &Rotation), Without<Sensor>>,
    links: Query<
        &PhysicsJointLink,
        (
            Without<JointDisabled>,
            Without<PhysicsJointPending>,
            Without<PhysicsJointTopologyPending>,
        ),
    >,
    graph: Res<ContactGraph>,
    time: Res<Time>,
    mut holds: ResMut<PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    if faults.first.is_some() || time.delta_secs_f64() <= 0.0 {
        return;
    }
    let dt = time.delta_secs_f64();
    let mut fractions = HashMap::<Entity, f64>::new();
    for pair in graph.iter_active() {
        if !pair.generates_constraints() {
            continue;
        }
        let Ok((shape1, owner1, pos1, rot1)) = colliders.get(pair.collider1) else {
            continue;
        };
        let Ok((shape2, owner2, pos2, rot2)) = colliders.get(pair.collider2) else {
            continue;
        };
        if owner1.body == owner2.body {
            continue;
        }
        let Ok([body1, body2]) = bodies.get_many([owner1.body, owner2.body]) else {
            continue;
        };
        let (_, kind1, body_pos1, body_rot1, com1, ccd1, solver1) = body1;
        let (_, kind2, body_pos2, body_rot2, com2, ccd2, solver2) = body2;
        let ccd1 = ccd1.filter(|ccd| !kind2.is_dynamic() || ccd.include_dynamic);
        let ccd2 = ccd2.filter(|ccd| !kind1.is_dynamic() || ccd.include_dynamic);
        if ccd1.is_none() && ccd2.is_none() {
            continue;
        }
        let motion1 = solved_motion(pos1, rot1, body_pos1, body_rot1, com1, solver1);
        let motion2 = solved_motion(pos2, rot2, body_pos2, body_rot2, com2, solver2);
        let linear_speed = (motion1.linvel - motion2.linvel).length() / dt;
        let angular_speed = (motion1.angvel - motion2.angvel).length() / dt;
        if ![ccd1, ccd2].into_iter().flatten().any(|ccd| {
            linear_speed >= ccd.linear_threshold || angular_speed >= ccd.angular_threshold
        }) {
            continue;
        }
        let nonlinear = [ccd1, ccd2]
            .into_iter()
            .flatten()
            .any(|ccd| ccd.mode == SweepMode::NonLinear);
        let hit = if nonlinear {
            cast_shapes_nonlinear(
                &motion1,
                shape1.shape_scaled().as_ref(),
                &motion2,
                shape2.shape_scaled().as_ref(),
                0.0,
                1.0,
                false,
            )
        } else {
            cast_shapes(
                &motion1.start,
                motion1.linvel,
                shape1.shape_scaled().as_ref(),
                &motion2.start,
                motion2.linvel,
                shape2.shape_scaled().as_ref(),
                ShapeCastOptions {
                    max_time_of_impact: 1.0,
                    stop_at_penetration: false,
                    ..default()
                },
            )
        };
        let hit = match hit {
            Ok(hit) => hit,
            Err(error) => {
                holds.set(PhysicsHolds::SAFETY_FAILURE, true);
                faults.raise(
                    "physics-ccd-unsupported",
                    Some(owner1.body),
                    "articulated CCD",
                    format!(
                        "sweep between {:?} and {:?} failed: {error:?}",
                        pair.collider1, pair.collider2
                    ),
                );
                return;
            }
        };
        if let Some(hit) = hit {
            let fraction = hit.time_of_impact;
            // Existing overlap belongs to the contact solver. CCD admits the
            // first positive contact; it must not rewind an already seated body.
            if fraction > 0.0 && fraction < 1.0 {
                for (body, kind) in [(owner1.body, kind1), (owner2.body, kind2)] {
                    if kind.is_dynamic() {
                        fractions
                            .entry(body)
                            .and_modify(|value| *value = value.min(fraction))
                            .or_insert(fraction);
                    }
                }
            }
        }
    }
    if fractions.is_empty() {
        return;
    }
    let islands = dynamic_joint_islands(
        bodies
            .iter()
            .filter(|(_, kind, _, _, _, _, solver)| kind.is_dynamic() && solver.is_some())
            .map(|(entity, ..)| entity),
        links.iter().map(|link| (link.body0, link.body1)),
    );
    for island in islands {
        let fraction = island
            .iter()
            .filter_map(|body| fractions.get(body))
            .copied()
            .fold(1.0, f64::min);
        if fraction >= 1.0 {
            continue;
        }
        for entity in island {
            if let Ok((_, _, _, _, _, _, Some(mut solver))) = bodies.get_mut(entity) {
                solver.delta_position *= fraction;
                solver.delta_rotation.0 = DQuat::IDENTITY.slerp(solver.delta_rotation.0, fraction);
            }
        }
    }
}
