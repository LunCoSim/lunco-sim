//! Unilateral suspension stops using Avian normal-contact impulses on both bodies.
use crate::{
    Suspension, WheelBodyMount, WheelRaycast, WheelRaycastResultsSet, raycast_contact_point,
    wheel_hub_pose,
};
use avian3d::dynamics::{
    joints::{EntityConstraint, joint_graph::JointGraphPlugin},
    solver::{
        ContactSoftnessCoefficients, SolverConfig,
        contact::ContactNormalPart,
        islands::BodyIslandNode,
        schedule::{SolverSystems, SubstepSolverSystems},
        solver_body::{SolverBodies, SolverBody, SolverBodyIndex, SolverBodyInertia},
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
            prepare_limits.in_set(SolverSystems::PrepareJoints),
        )
        .add_systems(
            SubstepSchedule,
            (
                solve_limits::<true, false>
                    .after(SubstepSolverSystems::WarmStart)
                    .before(SubstepSolverSystems::SolveConstraints),
                solve_limits::<false, true>
                    .after(SubstepSolverSystems::SolveConstraints)
                    .before(avian3d::dynamics::integrator::IntegrationSystems::Position),
                solve_limits::<false, false>
                    .after(SubstepSolverSystems::Relax)
                    .before(avian3d::dynamics::solver::xpbd::XpbdSolverSystems::SolveConstraints),
            ),
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
    contact: Option<ContactNormalPart>,
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

fn prepare_limits(
    bodies: Query<RigidBodyQueryReadOnly, Without<RigidBodyDisabled>>,
    mut limits: Query<(&RaycastTravelLimit, &mut TravelSolverData)>,
) {
    for (limit, mut data) in &mut limits {
        data.contact = None;
        let Ok(bodies) = bodies.get_many(limit.bodies) else {
            continue;
        };
        for i in 0..2 {
            data.anchors[i] = bodies[i].rotation * (limit.anchors[i] - bodies[i].center_of_mass.0);
        }
        data.center_difference = (bodies[1].position.0 - bodies[0].position.0).as_vec3()
            + bodies[1].rotation * bodies[1].center_of_mass.0
            - bodies[0].rotation * bodies[0].center_of_mass.0;
        data.normal = bodies[1].rotation * limit.normal;
    }
}

// A bump stop is a unilateral contact, not a position-projected joint. Reuse
// Avian's speculative normal impulse, softness, warm-start and relaxation
// contract. In particular, penetration correction must not become an
// unconstrained separating velocity through XPBD delta-position projection.
fn solve_limits<const WARM_START: bool, const USE_BIAS: bool>(
    mut solver_bodies: ResMut<SolverBodies>,
    body_indices: Query<Option<&SolverBodyIndex>, (With<RigidBody>, Without<RigidBodyDisabled>)>,
    mut limits: Query<(
        Entity,
        &lunco_physics::PhysicsOrderKey,
        &RaycastTravelLimit,
        &mut TravelSolverData,
    )>,
    time: Res<Time>,
    config: Res<SolverConfig>,
    softness: Res<ContactSoftnessCoefficients>,
    length_unit: Res<PhysicsLengthUnit>,
) {
    let mut order: Vec<_> = limits
        .iter()
        .map(|(e, key, ..)| (key.0.clone(), e))
        .collect();
    order.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let access = solver_bodies.access();
    for (_, entity) in order {
        let Ok((_, _, limit, mut data)) = limits.get_mut(entity) else {
            continue;
        };
        let Ok([ia, ib]) = body_indices.get_many(limit.bodies) else {
            continue;
        };
        let ia = ia.copied().unwrap_or(SolverBodyIndex::INVALID);
        let ib = ib.copied().unwrap_or(SolverBodyIndex::INVALID);
        if ia == ib {
            continue;
        }
        let mut dummy_a = SolverBody::default();
        let mut dummy_b = SolverBody::default();
        // SAFETY: endpoints are distinct and constraints run serially in stable wheel order.
        let (a, b) = unsafe { access.get_pair_unchecked_mut(ia, ib) };
        let (a, mut ia) = a.unwrap_or((&mut dummy_a, &SolverBodyInertia::DUMMY));
        let (b, mut ib) = b.unwrap_or((&mut dummy_b, &SolverBodyInertia::DUMMY));
        let dominance = ia.dominance() - ib.dominance();
        if dominance > 0 {
            ia = &SolverBodyInertia::DUMMY;
        }
        if dominance < 0 {
            ib = &SolverBodyInertia::DUMMY;
        }
        let [ra, rb] = data.anchors;
        let n = data.normal;
        let separation = (a.delta_position - b.delta_position + a.delta_rotation * ra
            - b.delta_rotation * rb
            - data.center_difference)
            .dot(n);
        let contact = data.contact.get_or_insert_with(|| {
            ContactNormalPart::generate(
                ia.effective_inv_mass() + ib.effective_inv_mass(),
                &ia.effective_inv_angular_inertia(),
                &ib.effective_inv_angular_inertia(),
                ra,
                rb,
                n,
                None,
                if dominance != 0 {
                    softness.non_dynamic
                } else {
                    softness.dynamic
                },
            )
        });
        let magnitude = if WARM_START {
            contact.impulse *= config.warm_start_coefficient;
            contact.total_impulse += contact.impulse;
            contact.impulse
        } else {
            contact.solve_impulse::<USE_BIAS>(
                separation,
                a.velocity_at_point(ra) - b.velocity_at_point(rb),
                n,
                config.max_overlap_solve_speed * length_unit.0,
                time.delta_secs(),
            )
        };
        let impulse = magnitude * n;
        a.linear_velocity += impulse * ia.effective_inv_mass();
        a.angular_velocity += ia.effective_inv_angular_inertia() * ra.cross(impulse);
        b.linear_velocity -= impulse * ib.effective_inv_mass();
        b.angular_velocity -= ib.effective_inv_angular_inertia() * rb.cross(impulse);
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
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
) {
    if step.delta().is_zero() {
        return;
    }
    let mut order: Vec<_> = wheels.iter().collect();
    order.sort_unstable_by(|a, b| a.1.0.cmp(&b.1.0));
    for (entity, _, wheel, susp, mount, hits, existing, data) in order {
        let load = data.filter(|_| existing.is_some()).map_or(0.0, |data| {
            data.contact.as_ref().map_or(0.0, |contact| {
                f64::from(contact.total_impulse) / step.delta_secs_f64()
            })
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
        // Speculative hits may be beyond the spring's rest length. Preserve
        // their positive free-travel gap instead of moving the stop to the
        // distant support; visual compression alone is clamped to zero.
        let signed_compression = susp.rest_length - f64::from(hit.distance);
        let limit_point =
            point + heading.0 * bevy::math::DVec3::Y * (susp.travel_distance - signed_compression);
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
    use bevy::math::DVec3;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    // Exercise the shared native integration, not a copy of its impulse law.
    // Deliberately late contacts reproduce the original energy-creation trigger
    // independently of any vehicle, spring tuning, raycast timing or Modelica.
    #[test]
    fn stop_impacts_do_not_create_energy_or_pull_separating_bodies() {
        // Fixture choices, not solver coefficients: a hard landing, an unequal
        // support mass, and upward platform motion exercise distinct responses.
        const IMPACT_SPEED_MPS: f32 = 30.0;
        const SUPPORT_MASS_RATIO: f32 = 2.0;
        const PLATFORM_SPEED_MPS: f32 = 2.0;
        const RADIUS_OF_GYRATION_M: f32 = 1.0;
        const DEPARTURE_SPEED_MPS: f32 = 10.0;
        const VELOCITY_TOLERANCE_MPS: f32 = 0.001;
        const SPECIFIC_ENERGY_TOLERANCE_J_PER_KG: f32 = 0.001;
        const STEP_SECONDS: f64 = 1.0 / 60.0;
        const OBSERVATION_FRAMES: usize = 12; // 0.2 s, including app startup.
        for carrier_mass in [1.0, 100.0, 2_000.0] {
            for penetration in [0.0, 0.05, 0.3] {
                for support_mode in [RigidBody::Static, RigidBody::Dynamic, RigidBody::Kinematic] {
                    for arm in [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.8)] {
                        let mut app = App::new();
                        app.init_resource::<Assets<Mesh>>()
                            .add_message::<AssetEvent<Mesh>>()
                            .add_plugins((MinimalPlugins, PhysicsPlugins::default()))
                            .init_resource::<lunco_physics::PhysicsHolds>()
                            .init_resource::<lunco_core::RuntimeFaults>()
                            .insert_resource(Gravity::ZERO)
                            .insert_resource(SubstepCount(8))
                            .insert_resource(Time::<Fixed>::from_seconds(STEP_SECONDS))
                            .insert_resource(TimeUpdateStrategy::ManualDuration(
                                Duration::from_secs_f64(STEP_SECONDS),
                            ));
                        install(&mut app);
                        app.finish();
                        app.cleanup();
                        let carrier_velocity = Vec3::NEG_Y * IMPACT_SPEED_MPS;
                        let support_mass = SUPPORT_MASS_RATIO * carrier_mass;
                        let support_velocity = if support_mode == RigidBody::Static {
                            Vec3::ZERO
                        } else {
                            Vec3::Y * PLATFORM_SPEED_MPS
                        };
                        let carrier_inertia = carrier_mass * RADIUS_OF_GYRATION_M.powi(2);
                        let support_inertia = support_mass * RADIUS_OF_GYRATION_M.powi(2);
                        let carrier = app
                            .world_mut()
                            .spawn((
                                RigidBody::Dynamic,
                                Mass(carrier_mass),
                                AngularInertia {
                                    principal: Vec3::splat(carrier_inertia),
                                    ..default()
                                },
                                NoAutoAngularInertia,
                                Position(DVec3::new(0.0, -penetration, 0.0)),
                                LinearVelocity(carrier_velocity),
                            ))
                            .id();
                        let support = app
                            .world_mut()
                            .spawn((
                                support_mode,
                                Mass(support_mass),
                                AngularInertia {
                                    principal: Vec3::splat(support_inertia),
                                    ..default()
                                },
                                NoAutoAngularInertia,
                                LinearVelocity(support_velocity),
                            ))
                            .id();
                        let stop = app
                            .world_mut()
                            .spawn((
                                lunco_physics::PhysicsOrderKey("stop".into()),
                                RaycastTravelLimit {
                                    bodies: [carrier, support],
                                    anchors: [arm; 2],
                                    normal: Vec3::Y,
                                },
                            ))
                            .id();
                        let initial_momentum =
                            carrier_mass * carrier_velocity + support_mass * support_velocity;
                        let initial_energy = 0.5 * carrier_mass * carrier_velocity.length_squared()
                            + 0.5 * support_mass * support_velocity.length_squared();
                        for _ in 0..OBSERVATION_FRAMES {
                            let before_v = app.world().get::<LinearVelocity>(carrier).unwrap().0;
                            let before_time = app.world().resource::<Time<Physics>>().elapsed();
                            app.update();
                            let v = app.world().get::<LinearVelocity>(carrier).unwrap().0;
                            let u = app.world().get::<LinearVelocity>(support).unwrap().0;
                            let omega = app.world().get::<AngularVelocity>(carrier).unwrap().0;
                            let support_omega =
                                app.world().get::<AngularVelocity>(support).unwrap().0;
                            assert!(v.is_finite() && u.is_finite());
                            if app.world().resource::<Time<Physics>>().elapsed() > before_time {
                                let data = app.world().get::<TravelSolverData>(stop).unwrap();
                                let impulse = data
                                    .contact
                                    .as_ref()
                                    .map_or(0.0, |contact| contact.total_impulse)
                                    * data.normal;
                                assert!(
                                    (impulse - carrier_mass * (v - before_v)).length()
                                        <= carrier_mass * VELOCITY_TOLERANCE_MPS,
                                    "reported stop impulse differs from momentum: carrier_mass={carrier_mass}, support={support_mode:?}, arm={arm:?}, penetration={penetration}, impulse={impulse:?}, before_v={before_v:?}, v={v:?}"
                                );
                            }
                            if support_mode == RigidBody::Dynamic {
                                let momentum = carrier_mass * v + support_mass * u;
                                assert!(
                                    (momentum - initial_momentum).length()
                                        <= carrier_mass * VELOCITY_TOLERANCE_MPS,
                                    "unequal reaction: carrier_mass={carrier_mass}, penetration={penetration}, v={v:?}, u={u:?}"
                                );
                                let energy = 0.5
                                    * (carrier_mass * v.length_squared()
                                        + carrier_inertia * omega.length_squared()
                                        + support_mass * u.length_squared()
                                        + support_inertia * support_omega.length_squared());
                                assert!(
                                    energy
                                        <= initial_energy
                                            + carrier_mass * SPECIFIC_ENERGY_TOLERANCE_J_PER_KG,
                                    "contact created energy: carrier_mass={carrier_mass}, penetration={penetration}, v={v:?}, u={u:?}"
                                );
                            } else {
                                // A moving kinematic support can do real work, but
                                // contact cannot add energy in its translating frame.
                                let relative_energy = 0.5
                                    * (carrier_mass * (v - support_velocity).length_squared()
                                        + carrier_inertia * omega.length_squared());
                                assert!(
                                    relative_energy
                                        <= 0.5
                                            * carrier_mass
                                            * (carrier_velocity - support_velocity)
                                                .length_squared()
                                            + carrier_mass * SPECIFIC_ENERGY_TOLERANCE_J_PER_KG,
                                    "artificial launch: support={support_mode:?}, carrier_mass={carrier_mass}, penetration={penetration}, v={v:?}"
                                );
                            }
                        }
                        let u = app.world().get::<LinearVelocity>(support).unwrap().0;
                        let stopped_v = app.world().get::<LinearVelocity>(carrier).unwrap().0;
                        assert!(
                            (stopped_v - carrier_velocity).length() > VELOCITY_TOLERANCE_MPS,
                            "the stop did not actually react to the impact"
                        );
                        if arm == Vec3::ZERO {
                            assert!(
                                (stopped_v - u).length() < VELOCITY_TOLERANCE_MPS,
                                "central impact did not settle"
                            );
                        }
                        let departing = u + DEPARTURE_SPEED_MPS
                            * (app.world().get::<Rotation>(support).unwrap().0 * Vec3::Y);
                        app.world_mut()
                            .get_mut::<LinearVelocity>(carrier)
                            .unwrap()
                            .0 = departing;
                        app.world_mut()
                            .get_mut::<AngularVelocity>(carrier)
                            .unwrap()
                            .0 = Vec3::ZERO;
                        app.world_mut()
                            .get_mut::<AngularVelocity>(support)
                            .unwrap()
                            .0 = Vec3::ZERO;
                        app.update();
                        let v = app.world().get::<LinearVelocity>(carrier).unwrap().0;
                        assert!(
                            (v - departing).length() < VELOCITY_TOLERANCE_MPS,
                            "a unilateral stop pulled a separating carrier"
                        );
                    }
                }
            }
        }
    }

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
