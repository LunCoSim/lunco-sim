//! Unilateral suspension stops, solved by Avian with both supporting bodies.
use crate::{
    Suspension, WheelBodyMount, WheelRaycast, WheelRaycastResultsSet, raycast_contact_point,
    wheel_hub_pose,
};
use avian3d::dynamics::{
    joints::{EntityConstraint, joint_graph::JointGraphPlugin},
    solver::{
        islands::BodyIslandNode,
        schedule::SolverSystems,
        solver_body::{SolverBody, SolverBodyInertia},
        xpbd::*,
    },
};
use avian3d::prelude::*;
use bevy::{
    ecs::entity::{EntityMapper, MapEntities},
    prelude::*,
};
use lunco_spatial::coords::{GridPos, GridRot};

pub(super) fn install(app: &mut App) {
    app.add_plugins(JointGraphPlugin::<RaycastTravelLimit>::default())
        .add_systems(
            FixedPostUpdate,
            sync_limits
                .after(WheelRaycastResultsSet)
                .run_if(lunco_physics::physics_is_live),
        )
        .add_systems(
            PhysicsSchedule,
            prepare_xpbd_joint::<RaycastTravelLimit>
                .in_set(SolverSystems::PreSubstep)
                .ambiguous_with_all(),
        )
        .add_systems(
            SubstepSchedule,
            solve_xpbd_joint::<RaycastTravelLimit>
                .in_set(XpbdSolverSystems::SolveUserConstraints)
                .after(crate::solve_differential_gear),
        );
}

#[derive(Component, Clone, Copy)]
#[require(TravelSolverData)]
pub(super) struct RaycastTravelLimit {
    bodies: [Entity; 2],
    anchors: [Vec3; 2],
    normal: Vec3,
}

#[derive(Component, Default)]
pub(super) struct RaycastStopLoad(pub f64);

#[derive(Component, Default)]
pub(super) struct TravelSolverData {
    anchors: [Vec3; 2],
    center_difference: Vec3,
    normal: Vec3,
    total_lagrange: Vec3,
}
impl XpbdConstraintSolverData for TravelSolverData {
    fn clear_lagrange_multipliers(&mut self) {
        self.total_lagrange = Vec3::ZERO;
    }
    fn total_position_lagrange(&self) -> Vec3 {
        self.total_lagrange
    }
}
impl EntityConstraint<2> for RaycastTravelLimit {
    fn entities(&self) -> [Entity; 2] {
        self.bodies
    }
}
impl MapEntities for RaycastTravelLimit {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        self.bodies = self.bodies.map(|body| mapper.get_mapped(body));
    }
}
impl PositionConstraint for RaycastTravelLimit {}
impl XpbdConstraint<2> for RaycastTravelLimit {
    type SolverData = TravelSolverData;
    fn prepare(&mut self, bodies: [&RigidBodyQueryReadOnlyItem; 2], data: &mut TravelSolverData) {
        for i in 0..2 {
            data.anchors[i] = bodies[i].rotation * (self.anchors[i] - bodies[i].center_of_mass.0);
        }
        data.center_difference = (bodies[1].position.0 - bodies[0].position.0).as_vec3()
            + bodies[1].rotation * bodies[1].center_of_mass.0
            - bodies[0].rotation * bodies[0].center_of_mass.0;
        data.normal = bodies[1].rotation * self.normal;
    }
    fn solve(
        &mut self,
        bodies: [&mut SolverBody; 2],
        inertias: [&SolverBodyInertia; 2],
        data: &mut TravelSolverData,
        dt: f32,
    ) {
        let [a, b] = bodies;
        let [ia, ib] = inertias;
        let ra = a.delta_rotation * data.anchors[0];
        let rb = b.delta_rotation * data.anchors[1];
        let n = b.delta_rotation * data.normal;
        // Normal points from the support toward the vehicle. Positive gap is free travel.
        let separation = b.delta_position - a.delta_position + rb - ra + data.center_difference;
        let penetration = separation.dot(n);
        if penetration <= 0.0 {
            return;
        }
        let wa = self.compute_generalized_inverse_mass(
            ia.effective_inv_mass().max_element(),
            ia.effective_inv_angular_inertia(),
            ra,
            n,
        );
        let wb = self.compute_generalized_inverse_mass(
            ib.effective_inv_mass().max_element(),
            ib.effective_inv_angular_inertia(),
            rb,
            n,
        );
        let impulse = compute_lagrange_update(0.0, -penetration, &[wa, wb], 0.0, dt) * n;
        data.total_lagrange += impulse;
        self.apply_positional_impulse(a, b, ia, ib, impulse, ra, rb);
    }
}

fn sync_limits(
    mut commands: Commands,
    wheels: Query<
        (
            Entity,
            &lunco_physics::PhysicsOrderKey,
            &WheelRaycast,
            &Suspension,
            &WheelBodyMount,
            &RayHits,
            Option<&RaycastTravelLimit>,
            Option<&TravelSolverData>,
        ),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    bodies: Query<
        (&Position, &Rotation, Has<BodyIslandNode>),
        (With<RigidBody>, Without<RigidBodyDisabled>),
    >,
    owners: Query<&ColliderOf>,
    mut visuals: Query<&mut Transform, Without<WheelRaycast>>,
    step: Res<Time<Physics>>,
    substep: Res<Time<Substeps>>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    if step.delta().is_zero() || substep.delta().is_zero() {
        return;
    }
    let mut order: Vec<_> = wheels.iter().collect();
    order.sort_unstable_by(|a, b| a.1.0.cmp(&b.1.0));
    for (entity, _, wheel, susp, mount, hits, existing, data) in order {
        let load = data.filter(|_| existing.is_some()).map_or(0.0, |data| {
            f64::from(data.total_lagrange.dot(data.normal).max(0.0))
                / (step.delta_secs_f64() * substep.delta_secs_f64())
        });
        commands.entity(entity).insert(RaycastStopLoad(load));
        let Ok((position, rotation, carrier_admitted)) = bodies.get(mount.body) else {
            commands
                .entity(entity)
                .remove::<(RaycastTravelLimit, TravelSolverData, RaycastStopLoad)>();
            continue;
        };
        let (hub, heading) = wheel_hub_pose(
            GridPos(position.0),
            GridRot(rotation.0.as_dquat()),
            mount.local.translation.as_dvec3(),
            mount.local.rotation.as_dquat() * wheel.heading_rotation,
        );
        let hit = hits
            .iter_sorted()
            .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
        let compression = hit.map_or(0.0, |h| (susp.rest_length - f64::from(h.distance)).max(0.0));
        // Presentation is derived from the completed pose/query, never clamped to hide overtravel.
        if let Some(visual) = wheel.visual_entity {
            if let Ok(mut transform) = visuals.get_mut(visual) {
                let y = compression as f32; // Explicit rendering boundary.
                if transform.translation.y != y {
                    transform.translation.y = y;
                }
            }
        }
        let Some(hit) = hit else {
            commands
                .entity(entity)
                .remove::<(RaycastTravelLimit, TravelSolverData, RaycastStopLoad)>();
            continue;
        };
        let support = owners
            .get(hit.entity)
            .map_or(hit.entity, |owner| owner.body);
        let Ok((support_position, support_rotation, support_admitted)) = bodies.get(support) else {
            commands
                .entity(entity)
                .remove::<(RaycastTravelLimit, TravelSolverData, RaycastStopLoad)>();
            continue;
        };
        // Static bodies have no solver node. Their rays and visuals remain valid,
        // but Avian can only admit a constraint with at least one native island.
        if support == mount.body || !(carrier_admitted || support_admitted) {
            commands
                .entity(entity)
                .remove::<(RaycastTravelLimit, TravelSolverData, RaycastStopLoad)>();
            continue;
        }
        let point = raycast_contact_point(
            hub.0,
            heading.0,
            susp.rest_length,
            wheel.wheel_radius,
            f64::from(hit.distance),
        );
        let limit_point =
            point + heading.0 * bevy::math::DVec3::Y * (susp.travel_distance - compression);
        let anchors = [
            rotation.0.as_dquat().inverse() * (limit_point - position.0),
            support_rotation.0.as_dquat().inverse() * (point - support_position.0),
        ];
        // Native solver scalar boundary: local lever arms and unit normals, not absolute positions.
        if !anchors
            .iter()
            .all(|p| lunco_physics::avian_backend_vector_is_valid(*p))
        {
            holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
            faults.raise(
                "mobility-native-travel-range-invalid",
                Some(mount.body),
                "suspension travel",
                "suspension stop lever exceeds the native solver range",
            );
            return;
        }
        let limit = RaycastTravelLimit {
            bodies: [mount.body, support],
            anchors: anchors.map(|p| p.as_vec3()),
            normal: support_rotation.0.inverse() * hit.normal,
        };
        commands.entity(entity).insert(limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::dynamics::joints::joint_graph::JointGraph;
    use bevy::ecs::system::RunSystemOnce;
    use std::time::Duration;

    #[test]
    fn travel_contacts_require_a_native_solver_endpoint() {
        for (carrier_mode, support_mode, expected) in [
            (RigidBody::Static, RigidBody::Static, false),
            (RigidBody::Dynamic, RigidBody::Static, true),
            (RigidBody::Static, RigidBody::Dynamic, true),
            (RigidBody::Kinematic, RigidBody::Static, true),
        ] {
            let mut app = App::new();
            app.init_resource::<Assets<Mesh>>()
                .add_message::<AssetEvent<Mesh>>()
                .add_plugins((MinimalPlugins, PhysicsPlugins::default()))
                .add_plugins(JointGraphPlugin::<RaycastTravelLimit>::default())
                .init_resource::<lunco_physics::PhysicsHolds>()
                .init_resource::<lunco_core::RuntimeFaults>();
            app.finish();
            app.cleanup();
            let carrier = app.world_mut().spawn(carrier_mode).id();
            let support = app.world_mut().spawn(support_mode).id();
            let wheel = app
                .world_mut()
                .spawn((
                    lunco_physics::PhysicsOrderKey("wheel".into()),
                    WheelRaycast::default(),
                    Suspension::default(),
                    WheelBodyMount {
                        body: carrier,
                        local: Transform::IDENTITY,
                    },
                    RayHits(vec![RayHitData {
                        entity: support,
                        distance: 0.2,
                        normal: Vec3::Y,
                    }]),
                ))
                .id();
            app.world_mut().flush();
            app.world_mut()
                .resource_mut::<Time<Physics>>()
                .advance_by(Duration::from_secs_f64(1.0 / 60.0));
            app.world_mut()
                .resource_mut::<Time<Substeps>>()
                .advance_by(Duration::from_secs_f64(1.0 / 480.0));
            app.world_mut().run_system_once(sync_limits).unwrap();
            assert_eq!(
                app.world().resource::<JointGraph>().get(wheel).is_some(),
                expected,
                "{carrier_mode:?} carrier on {support_mode:?} support"
            );
            app.world_mut().run_schedule(PhysicsSchedule);
        }
    }
}
