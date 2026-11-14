//! # Surface Mobility & Traction Physics
//!
//! This crate implements the core physics models for planetary rovers and
//! surface exploration vehicles.
//!
//! ## Ground interaction realizations
//! Traditional mesh-to-mesh collision for wheels is computationally expensive
//! and prone to "snagging" on terrain geometry. The crate supports two
//! realizations behind one authored tire law: raycast wheels use an analytical
//! suspension/contact point, while jointed wheels use Avian for normal contact
//! and the revolute constraint.
//! 1. **Suspension Logic**: Raycast wheels compute normal forces from their
//!    authored spring and the measured ray stroke; jointed suspension uses the
//!    standard USD prismatic-drive projection and Avian's native solver.
//! 2. **Traction Physics**: Both realizations apply the same longitudinal and
//!    lateral tire equations; only the source of the normal load and contact
//!    point differs.
//! 3. **Numeric Stability**: The raycast realization projects a single ray to
//!    avoid wheel snagging on irregular procedural terrain.
//!
//! Motion allocation is not a Rust policy. Modelica/Rhai programs publish the
//! final drive torques and wheel headings through generic ports; this crate
//! realizes those values as wheel, suspension, tire, and brake mechanics.

use avian3d::dynamics::solver::{
    schedule::SolverSystems,
    solver_body::{SolverBodies, SolverBody, SolverBodyIndex, SolverBodyInertia},
};
use avian3d::prelude::*;
use bevy::ecs::schedule::common_conditions::any_with_component;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use lunco_core::AppSceneExt;
use lunco_core::MobilityRoot;
use lunco_physics::drive::{CoordinateDrive, integrate_coordinate_drive};
use lunco_port_core::Port;
use lunco_port_core::{InputPorts, OutputPorts};
use lunco_spatial::coords::{GridPos, GridRot};
use std::borrow::Borrow;
use std::collections::HashSet;

mod jointed_tire;
mod raycast_travel;
mod sensing;
mod wheel_spin;
pub use jointed_tire::{JointedWheelTire, apply_jointed_tire_forces};
use wheel_spin::update_wheel_spin;

/// Orient an Avian contact normal so it points from the support surface toward
/// the requested body. Avian stores manifold normals from collider 1 to
/// collider 2; every wheel-contact consumer must use this one convention.
pub fn contact_normal_for_body(
    pair: &ContactPair,
    manifold: &ContactManifold,
    body: Entity,
) -> Option<DVec3> {
    if pair.body1 == Some(body) {
        Some(-manifold.normal.as_dvec3())
    } else if pair.body2 == Some(body) {
        Some(manifold.normal.as_dvec3())
    } else {
        None
    }
}

pub mod wheel_kinematics;
use wheel_kinematics::wheel_hub_pose;

/// Child endpoints for physical loss rates, in W. The force owner publishes
/// these samples; authored thermal networks consume them through PortSurface.
/// Scene hierarchy owns endpoint retirement together with the wheel.
#[derive(Component, Clone, Copy, Debug, Reflect)]
#[reflect(Component)]
pub struct WheelHeatPorts {
    pub bearing: Entity,
    pub brake: Entity,
    /// Present only when the wheel's tire-contact owner publishes physical work.
    pub tire: Option<Entity>,
    /// Actual support contact (0 or 1), consumed by thermal contact conductance.
    pub contact: Option<Entity>,
    /// Loaded contact with fixed world support, rather than another dynamic
    /// body. Published by the collider-backed tire owner only.
    pub fixed_contact: Option<Entity>,
}

/// Manages the integration of mobility physics and control observers.
pub struct LunCoMobilityPlugin;

/// Post-step boundary after the mobility-owned wheel ray hits are current.
/// Readers of `RayHits` in `FixedPostUpdate` can order after this set.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WheelRaycastResultsSet;

/// Dynamic bodies held by an authored fixed weld during the current mobility
/// force pass. The ordered fixed-step chain refreshes this once and shares it
/// with the suspension and tire-force systems.
#[derive(Resource, Default)]
struct DynamicFixedBodies(HashSet<Entity>);

fn mark_wheel_ports_causal(
    trigger: On<Add<WheelRaycast>>,
    query: Query<&WheelRaycast>,
    mut commands: Commands,
) {
    let Ok(wheel) = query.get(trigger.entity) else {
        return;
    };
    for port in [wheel.drive_port, wheel.heading_port] {
        if port != Entity::PLACEHOLDER {
            commands
                .entity(port)
                .try_insert(lunco_port_core::CausalStateSink);
        }
    }
}

/// Requests a rebuild of the owning vehicle's derived raycast support geometry.
/// USD projection and authored wheel resync stamp this marker when source
/// geometry changes; the publisher consumes it after rebuilding a complete
/// root footprint.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct RaycastSupportGeometryDirty;

fn mark_raycast_support_geometry_dirty(trigger: On<Add<WheelRaycast>>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(RaycastSupportGeometryDirty);
}

impl Plugin for LunCoMobilityPlugin {
    fn build(&self, app: &mut App) {
        // Expose physics-backed spatial queries (Raycast, GroundHeight) so the
        // API / MCP / rhai `query()` can sense geometry without depending on avian.
        sensing::register_physics_queries(app);
        raycast_travel::install(app);
        // Bridge avian collision / trigger-volume events onto the telemetry bus
        // so scripts can react via `on_event` instead of polling distance().
        sensing::register_collision_event_bridge(app);

        app.init_scene_resource::<DynamicFixedBodies>()
            .init_scene_resource::<DifferentialSolverOrder>()
            .add_systems(
                PhysicsSchedule,
                prepare_differential_order
                    .after(SolverSystems::PrepareSolverBodies)
                    .before(SolverSystems::PrepareJoints),
            )
            .register_type::<Suspension>()
            .register_type::<WheelRaycast>()
            .register_type::<WheelHeatPorts>()
            .register_type::<JointedWheelTire>()
            .register_type::<TireLateralStiffnessGraph>()
            .register_type::<DifferentialCoupling>()
            .register_type::<SuspensionPiston>()
            .register_type::<SuspensionSpring>()
            .register_type::<WheelBodyMount>()
            .register_type::<RaycastMassContribution>()
            .add_observer(mark_wheel_ports_causal)
            .add_observer(mark_raycast_support_geometry_dirty)
            // A vehicle's mass must not depend on which `drivetrain` variant
            // realizes its wheels. Mass preparation must land even while
            // physics is held — a rover that spawns during a cinematic hold is
            // still the same rover.
            .add_systems(FixedUpdate, fold_raycast_wheel_mass)
            // A raycast suspension is a physics model without Avian colliders.
            // Publish its support geometry through the physics contract once the
            // USD-to-mobility projection has created the wheel entities. Terrain
            // consumes only that contract and never inspects mobility types.
            .add_systems(
                Update,
                publish_raycast_support_footprints
                    .in_set(lunco_physics::PhysicsSupportSet::Publish),
            )
            .add_systems(
                Update,
                initialize_raycast_wheel_contact_diagnostics
                    .after(publish_raycast_support_footprints),
            )
            // Raycast ownership is topology-driven. Refresh each caster's
            // exclusion set when its caster is admitted or Avian's joint graph
            // changes, so articulated suspensions ignore every collider in
            // their connected assembly rather than only the immediate parent.
            .add_systems(
                Update,
                sync_raycast_assembly_filters
                    .after(publish_raycast_support_footprints)
                    .after(initialize_raycast_wheel_contact_diagnostics),
            )
            // G5 rocker-bogie differential — separate set: it doesn't read the
            // control ports, only couples two rocker hinges. Idle unless a
            // `DifferentialCoupling` exists, so it's free for every other vehicle.
            .add_systems(
                // The differential is a substep constraint. It runs before the
                // native revolute pass so native hinge alignment and limits
                // close the same XPBD iteration.
                SubstepSchedule,
                solve_differential_gear
                    .in_set(
                        avian3d::dynamics::solver::xpbd::XpbdSolverSystems::SolveUserConstraints,
                    )
                    .run_if(any_with_component::<DifferentialCoupling>)
                    // Same live-physics gate as the wheel systems: a frozen scene
                    // must not have its linkage projected while it is mounting.
                    .run_if(lunco_physics::physics_is_live),
            )
            .add_systems(
                FixedUpdate,
                (
                    refresh_dynamically_fixed_bodies,
                    apply_wheel_suspension,
                    update_raycast_support_state,
                    update_suspension_visuals,
                    // HEADING, then SOLVE THE TIRE, then APPLY IT. Heading first so the
                    // contact basis is this tick's heading; the spin solve produces the
                    // patch force; `apply_wheel_drive` only hands it to the body. The
                    // previous ordering (drive → heading → spin) meant the chassis
                    // force was built from last tick's heading and from a spin it could not
                    // see, which is what forced the two independent force fudges.
                    apply_wheel_heading,
                    update_wheel_spin,
                    apply_wheel_drive,
                    publish_raycast_wheel_contact_diagnostics,
                    apply_jointed_tire_forces,
                )
                    .chain()
                    // Read the actuator `Port` AFTER wire propagation has carried this
                    // tick's command into it (same fixed tick), so actuation isn't
                    // delayed an extra tick. See `lunco_core_runtime::ControlDacSet`.
                    .after(lunco_core_runtime::ControlDacSet)
                    .run_if(
                        // Run wherever physics is live. On a pure client this used to be
                        // skipped entirely (replicated rovers are server-authoritative
                        // proxies); predict-own now lets the client locally simulate the
                        // ONE rover it possesses. We don't gate by role here — the owned
                        // rover is the only `Dynamic` chassis on a client (every other
                        // replicated body is pinned `Kinematic` by `force_kinematic_proxies`),
                        // and the per-chassis `RigidBody::Kinematic` guard inside each wheel
                        // system already skips those. So host/standalone simulate every
                        // rover (unchanged) and a client simulates only its owned one.
                        //
                        // Transient forces are consumed only by an admitted native step.
                        lunco_physics::physics_is_live,
                    ),
            );
        // Keep command-surface synchronization and braking in the generic
        // substrate. Drive allocation is an authored Modelica/Rhai network;
        // there is deliberately no Rust vehicle policy between these ports and
        // the generic cosimulation propagation pass.
        app.add_systems(
            FixedUpdate,
            (sync_input_ports, apply_vehicle_brake)
                .chain()
                .before(lunco_core_runtime::ControlDacSet),
        );

        // Keep raycast wheels' physics `Position`/`Rotation` grid-absolute so their
        // suspension rays originate in avian's frame (not the big_space render
        // frame) — the fix for "rover rests but won't drive at an elevated site".
        // Runs in the physics schedule AFTER the step (fresh chassis pose) and
        // BEFORE the spatial query casts the rays. See the fn docs.
        app.add_systems(
            FixedPostUpdate,
            sync_raycast_wheel_physics_pose
                .after(PhysicsSystems::StepSimulation)
                .before(SpatialQuerySystems),
        );
        // Avian keeps ownership of caster pose propagation and the shared
        // SpatialQuery implementation. Suspend only mobility wheel casts in
        // its serial component loop, then recast through the same query API
        // after Avian has refreshed global ray poses. Other casters stay native.
        app.add_systems(
            FixedPostUpdate,
            suppress_native_wheel_raycasts
                .after(sync_raycast_wheel_physics_pose)
                .before(SpatialQuerySystems)
                .run_if(any_with_component::<WheelRaycast>),
        )
        .add_systems(
            FixedPostUpdate,
            cast_wheel_ray_hits_parallel
                .after(SpatialQuerySystems)
                .in_set(WheelRaycastResultsSet)
                .run_if(any_with_component::<WheelRaycast>),
        );

        // ── Rollback replay ──────────────────────────────────────────────────
        // Mirror the FULL actuation chain into `RollbackReplay` with the SAME
        // relative order as `FixedUpdate`, so re-simulating a recorded input
        // reproduces the host's forces exactly. No `Time<Virtual>` pause guard here:
        // a replay step is an instantaneous re-simulation, not a wall-clock tick, so
        // it must run regardless of the pause/speed state of the virtual clock.
        app.add_systems(
            lunco_core_runtime::RollbackReplay,
            (sync_input_ports, apply_vehicle_brake)
                .chain()
                .before(lunco_core_runtime::ControlDacSet),
        );
        app.add_systems(
            lunco_core_runtime::RollbackReplay,
            (
                refresh_dynamically_fixed_bodies,
                apply_wheel_suspension,
                update_raycast_support_state,
                update_suspension_visuals,
                // HEADING, then SOLVE THE TIRE, then APPLY IT. Heading first so the
                // contact basis is this tick's heading; the spin solve produces the
                // patch force; `apply_wheel_drive` only hands it to the body. The
                // previous ordering (drive → heading → spin) meant the chassis
                // force was built from last tick's heading and from a spin it could not
                // see, which is what forced the two independent force fudges.
                apply_wheel_heading,
                update_wheel_spin,
                apply_wheel_drive,
            )
                .chain()
                .after(lunco_core_runtime::ControlDacSet),
        );
    }
}

/// Keep a raycast suspension out of the complete joint-connected assembly.
///
/// `RayCaster`'s filter is expressed in collider entities, while the Avian
/// joint graph is expressed in rigid-body entities. The projection below is
/// the one generic bridge between those two existing runtime products: first
/// resolve the carrier's connected bodies from Avian's graph, then add every
/// collider owned by one of those bodies to the caster's filter. No vehicle
/// path, name, or drivetrain variant participates in the result.
fn sync_raycast_assembly_filters(
    joints: Option<Res<avian3d::dynamics::joints::joint_graph::JointGraph>>,
    mut raycasters: Query<
        (
            &WheelBodyMount,
            &mut RayCaster,
            Option<&mut lunco_physics::PhysicsWheelRaycastFilter>,
        ),
        With<WheelRaycast>,
    >,
    colliders: Query<(Entity, Option<&ColliderOf>), With<Collider>>,
    travel_contacts: Query<(), With<raycast_travel::RaycastTravelLimit>>,
    changed_colliders: Query<Entity, Or<(Added<Collider>, Changed<ColliderOf>)>>,
    removed_colliders: RemovedComponents<Collider>,
) {
    let graph_changed = joints.as_ref().is_some_and(Res::is_changed);
    let collider_topology_changed = !changed_colliders.is_empty() || !removed_colliders.is_empty();
    for (mount, mut raycaster, mut filter_snapshot) in &mut raycasters {
        let filter_added = filter_snapshot
            .as_ref()
            .is_some_and(|filter| filter.is_added());
        if !graph_changed && !collider_topology_changed && !raycaster.is_added() && !filter_added {
            continue;
        }

        let carrier = mount.body;
        let mut members = HashSet::from([carrier]);
        let mut pending = vec![carrier];
        if let Some(joints) = joints.as_deref() {
            while let Some(body) = pending.pop() {
                for edge in joints.joints_of(body) {
                    if travel_contacts.contains(edge.entity) {
                        continue;
                    }
                    let connected = if edge.body1 == body {
                        edge.body2
                    } else {
                        edge.body1
                    };
                    if members.insert(connected) {
                        pending.push(connected);
                    }
                }
            }
        }

        raycaster.query_filter.excluded_entities.clear();
        raycaster
            .query_filter
            .excluded_entities
            .extend(members.iter().copied());
        for (collider, owner) in &colliders {
            let body = owner.map_or(collider, |owner| owner.body);
            if members.contains(&body) {
                raycaster.query_filter.excluded_entities.insert(collider);
            }
        }

        // Keep this allocation outside FixedUpdate. The contact publisher
        // only reads the snapshot while producing its in-place sample.
        if let Some(mut filter_snapshot) = filter_snapshot.take() {
            let mut excluded_entities = raycaster
                .query_filter
                .excluded_entities
                .iter()
                .copied()
                .collect::<Vec<_>>();
            excluded_entities.sort_by_key(|entity| entity.to_bits());
            filter_snapshot.excluded_entities = excluded_entities;
            filter_snapshot.signature =
                raycast_filter_signature(&raycaster.query_filter.excluded_entities);
        }
    }
}

/// Hash the small native exclusion set without allocating. The signature is
/// order-independent because the source is a `HashSet`; the exact sorted
/// members are copied only when this value changes.
fn raycast_filter_signature<I>(excluded_entities: I) -> u64
where
    I: IntoIterator,
    I::Item: Borrow<Entity>,
{
    excluded_entities
        .into_iter()
        .fold(0u64, |signature, entity| {
            let entity = entity.borrow();
            let bits = entity.to_bits();
            let mixed = bits ^ bits.rotate_left(29) ^ 0x9e37_79b9_7f4a_7c15;
            signature.wrapping_add(mixed.rotate_left((bits & 63) as u32))
        })
}

/// Independent carrier baseline and the last admitted ordered source set.
/// Native aggregate overrides must never become their own next baseline.
#[derive(Component, Debug, Clone)]
pub struct ReducedMassState {
    baseline: avian3d::parry::mass_properties::MassProperties,
    sources: Vec<(Entity, avian3d::parry::mass_properties::MassProperties)>,
    automatic: (bool, bool, bool),
    geometry_owner: Option<lunco_physics::body_mass::BodyMassFromColliders>,
    inputs: ReducedComputedInputs,
}

/// Computed co-simulation inputs are complete body properties, independent of
/// the aggregate's native caches. A missing cache identifies that typed owner.
#[derive(Clone, Debug, Default, PartialEq)]
struct ReducedComputedInputs {
    mass: Option<ComputedMass>,
    inertia: Option<ComputedAngularInertia>,
    center: Option<ComputedCenterOfMass>,
}

impl ReducedComputedInputs {
    #[allow(clippy::too_many_arguments)]
    fn read(
        mass: Option<&Mass>,
        computed_mass: Option<&ComputedMass>,
        mass_override: bool,
        inertia: Option<&AngularInertia>,
        computed_inertia: Option<&ComputedAngularInertia>,
        inertia_override: bool,
        center: Option<&CenterOfMass>,
        computed_center: Option<&ComputedCenterOfMass>,
        center_override: bool,
    ) -> Self {
        Self {
            mass: (mass_override && mass.is_none())
                .then(|| computed_mass.copied())
                .flatten(),
            inertia: (inertia_override && inertia.is_none())
                .then(|| computed_inertia.copied())
                .flatten(),
            center: (center_override && center.is_none())
                .then(|| computed_center.copied())
                .flatten(),
        }
    }
}

/// The authored rigid body that owns a wheel realization and the wheel's pose
/// in that body's frame. USD hierarchy is allowed to contain non-body carrier
/// prims, so a wheel's direct `ChildOf` is not a physics ownership contract.
/// The USD projector resolves this once from composed body topology; mobility
/// systems consume the result without path or name rules.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct WheelBodyMount {
    pub body: Entity,
    pub local: lunco_core::DTransform,
}

/// Mass and authored principal inertia of a physical child that is absent as a
/// rigid body in the active reduced realization. The USD projector resolves the
/// owner and body-local pose; the fold is then the same composed mass-property
/// operation as the existing wheel fold.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct RaycastMassContribution {
    pub owner: Entity,
    pub order_key: lunco_physics::PhysicsOrderKey,
    pub local: lunco_core::DTransform,
    pub mass: f64,
    /// Centre of mass in the contributing prim's canonical local frame.
    pub center_of_mass: DVec3,
    pub principal: DVec3,
    /// Principal frame in the contributing prim's canonical local frame.
    pub principal_axes: DQuat,
}

/// Resolve a wheel's authored carrier chain to the vehicle's actuator owner.
///
/// A wheel can be a direct child of the vessel root or hang from an articulated
/// link such as a rocker or bogie.  The mobility systems that apply forces use
/// the immediate rigid-body carrier; mass and initial-support publication need
/// the inverse mapping so their body-local offsets can be expressed in the
/// vessel owner's frame.  The authored `ChildOf` chain is the topology source;
/// no vehicle-specific path spelling belongs here.
fn actuator_root_and_local_transform(
    wheel: Entity,
    roots: &Query<Entity, With<MobilityRoot>>,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Option<(Entity, Transform)> {
    let mut current = wheel;
    let mut chain = Vec::new();
    let mut visited = HashSet::new();

    loop {
        if roots.get(current).is_ok() {
            let mut local = Transform::IDENTITY;
            for transform in chain.iter().rev() {
                local = local.mul_transform(*transform);
            }
            return Some((current, local));
        }
        if !visited.insert(current) {
            return None;
        }
        chain.push(transforms.get(current).copied().unwrap_or_default());
        current = parents.get(current).ok()?.parent();
    }
}

/// Publish the support envelope of the raycast physics realization.
///
/// This is intentionally owned by mobility: it knows the contact model and its
/// wheel parameters. The published component is owned by `lunco-physics`, so
/// terrain, readiness, and future support consumers do not depend on rover
/// components. Collider-backed bodies need no publisher; Avian supplies their
/// support geometry through runtime collider AABBs.
fn publish_raycast_support_footprints(
    mut commands: Commands,
    roots: Query<Option<&lunco_physics::PhysicsSupportFootprint>, With<MobilityRoot>>,
    actuator_roots: Query<Entity, With<MobilityRoot>>,
    dirty_wheels: Query<Entity, With<RaycastSupportGeometryDirty>>,
    wheels: Query<(Entity, &WheelRaycast, &Suspension)>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
) {
    let mut affected_roots = HashSet::new();
    let mut resolved_dirty_wheels = Vec::new();
    for wheel in &dirty_wheels {
        if let Some((root, _)) =
            actuator_root_and_local_transform(wheel, &actuator_roots, &parents, &transforms)
        {
            affected_roots.insert(root);
            resolved_dirty_wheels.push(wheel);
        }
    }
    if affected_roots.is_empty() {
        return;
    }

    let mut contacts_by_root = std::collections::HashMap::new();
    for (wheel_entity, wheel, suspension) in &wheels {
        let Some((root, local)) =
            actuator_root_and_local_transform(wheel_entity, &actuator_roots, &parents, &transforms)
        else {
            continue;
        };
        if !affected_roots.contains(&root)
            || !wheel.wheel_radius.is_finite()
            || wheel.wheel_radius <= 0.0
        {
            continue;
        }
        contacts_by_root.entry(root).or_insert_with(Vec::new).push(
            lunco_physics::PhysicsSupportContact {
                local_offset: local.translation.as_dvec3(),
                radius: wheel.wheel_radius,
                probe_origin: local.translation.as_dvec3()
                    + local.rotation.as_dquat()
                        * DVec3::Y
                        * strut_offset(suspension.rest_length, wheel.wheel_radius),
                probe_direction: local.rotation.as_dquat() * DVec3::NEG_Y,
                probe_length: suspension.rest_length,
            },
        );
    }

    for root in affected_roots {
        let Ok(existing) = roots.get(root) else {
            continue;
        };
        let mut contacts = contacts_by_root.remove(&root).unwrap_or_default();
        contacts.sort_by(|a, b| {
            a.local_offset
                .x
                .total_cmp(&b.local_offset.x)
                .then_with(|| a.local_offset.y.total_cmp(&b.local_offset.y))
                .then_with(|| a.local_offset.z.total_cmp(&b.local_offset.z))
                .then_with(|| a.radius.total_cmp(&b.radius))
                .then_with(|| a.probe_origin.x.total_cmp(&b.probe_origin.x))
                .then_with(|| a.probe_origin.y.total_cmp(&b.probe_origin.y))
                .then_with(|| a.probe_origin.z.total_cmp(&b.probe_origin.z))
                .then_with(|| a.probe_direction.x.total_cmp(&b.probe_direction.x))
                .then_with(|| a.probe_direction.y.total_cmp(&b.probe_direction.y))
                .then_with(|| a.probe_direction.z.total_cmp(&b.probe_direction.z))
                .then_with(|| a.probe_length.total_cmp(&b.probe_length))
        });

        if contacts.is_empty() {
            if existing.is_some() {
                commands
                    .entity(root)
                    .remove::<lunco_physics::PhysicsSupportFootprint>()
                    .remove::<lunco_physics::PhysicsSupportState>();
            }
        } else {
            let footprint = lunco_physics::PhysicsSupportFootprint(contacts);
            if existing.is_none_or(|current| current != &footprint) {
                commands
                    .entity(root)
                    .try_insert((footprint, lunco_physics::PhysicsSupportState::default()));
            }
        }
    }

    for wheel in resolved_dirty_wheels {
        commands
            .entity(wheel)
            .remove::<RaycastSupportGeometryDirty>();
    }
}

/// Publish the live contact result for the raycast support footprint.
///
/// The footprint itself is authored geometry and is intentionally stable after
/// publication. Contact is a fixed-step observation of the current ray hits,
/// so it is updated separately after suspension has consumed the same hits.
/// Keeping the two values separate prevents a consumer from treating a wheel
/// probe as grounded merely because the probe exists.
fn update_raycast_support_state(
    wheels: Query<(&WheelRaycast, &Suspension, &RayHits, &WheelBodyMount)>,
    mut roots: Query<&mut lunco_physics::PhysicsSupportState, With<MobilityRoot>>,
    tick: Res<lunco_core_runtime::SimTick>,
) {
    for mut state in &mut roots {
        state.active_contact_count = 0;
        state.sample_tick = tick.0;
    }

    for (_wheel, suspension, hits, mount) in &wheels {
        let grounded = hits.iter_sorted().any(|hit| {
            hit.distance.is_finite()
                && hit.distance >= 0.0
                && hit.normal.is_finite()
                && hit.normal.length_squared() > 1.0e-12
                && f64::from(hit.distance) < suspension.rest_length
        });
        if grounded {
            if let Ok(mut state) = roots.get_mut(mount.body) {
                state.active_contact_count = state.active_contact_count.saturating_add(1);
            }
        }
    }
}

/// Allocate the per-wheel diagnostic component once after USD projection.
/// Fixed-step publication then mutates it in place, avoiding a deferred ECS
/// command and an archetype lookup for every wheel on every sample.
fn initialize_raycast_wheel_contact_diagnostics(
    mut commands: Commands,
    wheels: Query<(Entity, &WheelBodyMount), Without<lunco_physics::PhysicsWheelContact>>,
) {
    for (wheel_entity, mount) in &wheels {
        commands
            .entity(wheel_entity)
            .insert(lunco_physics::PhysicsWheelContact {
                owner: mount.body,
                contact_valid: false,
                hit_entity: None,
                distance_m: None,
                normal: DVec3::ZERO,
                normal_force_n: 0.0,
                suspension_rest_length_m: 0.0,
                suspension_compression_m: 0.0,
                tire_force: DVec3::ZERO,
                ray_hit_count: 0,
                valid_ray_hit_count: 0,
                raycast_filter_excluded_entity_count: 0,
                ray_origin: DVec3::ZERO,
                ray_direction: DVec3::ZERO,
                ray_max_distance_m: 0.0,
                sample_tick: 0,
            });
        commands
            .entity(wheel_entity)
            .insert(lunco_physics::PhysicsWheelRaycastFilter::default());
    }
}

/// Publish the effective native raycast result after suspension and tire force
/// application. This is deliberately a snapshot component in `lunco-physics`:
/// scene queries can expose it without coupling to this mobility realization,
/// and a future wheel implementation can publish the same evidence contract.
fn publish_raycast_wheel_contact_diagnostics(
    mut wheels: Query<(
        &WheelRaycast,
        &Suspension,
        &RayHits,
        &WheelBodyMount,
        Option<&RayCaster>,
        Option<&mut lunco_physics::PhysicsWheelRaycastFilter>,
        &mut lunco_physics::PhysicsWheelContact,
    )>,
    tick: Res<lunco_core_runtime::SimTick>,
) {
    for (wheel, suspension, hits, mount, raycaster, mut filter_snapshot, mut snapshot) in
        &mut wheels
    {
        if let (Some(raycaster), Some(mut filter_snapshot)) = (raycaster, filter_snapshot.take()) {
            let signature = raycast_filter_signature(&raycaster.query_filter.excluded_entities);
            if signature != filter_snapshot.signature
                || filter_snapshot.excluded_entities.len()
                    != raycaster.query_filter.excluded_entities.len()
            {
                let mut excluded_entities = raycaster
                    .query_filter
                    .excluded_entities
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                excluded_entities.sort_by_key(|entity| entity.to_bits());
                filter_snapshot.excluded_entities = excluded_entities;
                filter_snapshot.signature = signature;
            }
        }
        let selected = hits.0.iter().filter(|hit| {
            hit.distance.is_finite()
                && hit.distance >= 0.0
                && hit.normal.is_finite()
                && hit.normal.length_squared() > 1.0e-12
        });
        let hit = selected.min_by(|left, right| {
            left.distance
                .partial_cmp(&right.distance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let (hit_entity, distance_m, normal, contact_valid) = match hit {
            Some(hit) => (
                Some(hit.entity),
                Some(f64::from(hit.distance)),
                hit.normal.as_dvec3(),
                f64::from(hit.distance) < suspension.rest_length,
            ),
            None => (None, None, DVec3::ZERO, false),
        };
        let compression = distance_m
            .filter(|distance| *distance < suspension.rest_length)
            .map(|distance| (suspension.rest_length - distance).max(0.0))
            .unwrap_or(0.0);
        let valid_ray_hit_count = hits
            .0
            .iter()
            .filter(|hit| {
                hit.distance.is_finite()
                    && hit.distance >= 0.0
                    && hit.normal.is_finite()
                    && hit.normal.length_squared() > 1.0e-12
            })
            .count() as u32;
        snapshot.owner = mount.body;
        snapshot.contact_valid = contact_valid;
        snapshot.hit_entity = hit_entity;
        snapshot.distance_m = distance_m;
        snapshot.normal = normal;
        snapshot.normal_force_n = wheel.last_normal_force;
        snapshot.suspension_rest_length_m = suspension.rest_length;
        snapshot.suspension_compression_m = compression;
        snapshot.tire_force = wheel.tire_force;
        snapshot.ray_hit_count = hits.0.len() as u32;
        snapshot.valid_ray_hit_count = valid_ray_hit_count;
        snapshot.raycast_filter_excluded_entity_count = raycaster
            .map(|raycaster| raycaster.query_filter.excluded_entities.len() as u32)
            .unwrap_or(0);
        snapshot.ray_origin = raycaster
            .map(|raycaster| raycaster.global_origin())
            .unwrap_or(DVec3::ZERO);
        snapshot.ray_direction = raycaster
            .map(|raycaster| raycaster.global_direction().as_dvec3())
            .unwrap_or(DVec3::ZERO);
        snapshot.ray_max_distance_m = raycaster
            .map(|raycaster| f64::from(raycaster.max_distance))
            .unwrap_or(0.0);
        snapshot.sample_tick = tick.0;
    }
}

/// Recompute reduced mass from an independent baseline when its source set changes.
///
/// Parry reduces complete body-frame tensors and centres of mass in stable
/// source order. A proxy wheel contributes its mass at the suspension mount;
/// its independent spin inertia remains owned by the wheel integrator. Other
/// physical children retain their authored COM, tensor and principal frame.
///
/// The complete native candidate is validated before any component changes.
/// Missing preparation or invalid physical data raises a runtime fault and
/// holds integration rather than publishing a partial mass-property update.
pub fn fold_raycast_wheel_mass(
    mut commands: Commands,
    q_wheels: Query<(
        Entity,
        &WheelRaycast,
        &WheelBodyMount,
        Option<&lunco_physics::PhysicsOrderKey>,
    )>,
    q_contributions: Query<(Entity, &RaycastMassContribution)>,
    changed_contributions: Query<Entity, Changed<RaycastMassContribution>>,
    changed_mounts: Query<Entity, Changed<WheelBodyMount>>,
    changed_keys: Query<Entity, Changed<lunco_physics::PhysicsOrderKey>>,
    mut removed_keys: RemovedComponents<lunco_physics::PhysicsOrderKey>,
    mut q_body: Query<
        (
            Entity,
            Option<&mut Mass>,
            Option<&ComputedMass>,
            Has<NoAutoMass>,
            Option<&mut AngularInertia>,
            Has<NoAutoAngularInertia>,
            Option<&CenterOfMass>,
            Option<&ComputedCenterOfMass>,
            Option<&ComputedAngularInertia>,
            Has<NoAutoCenterOfMass>,
            Option<&ReducedMassState>,
            Option<&lunco_physics::body_mass::BodyMassFromColliders>,
        ),
        Or<(With<RigidBody>, With<Mass>, With<ComputedMass>)>,
    >,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: Option<ResMut<Time<Physics>>>,
) {
    use avian3d::parry::{mass_properties::MassProperties, math::Pose};
    use lunco_physics::body_mass::{checked_mass_properties, native_mass_properties};

    // WheelRaycast also contains continuously updated spin/contact state.
    // Compare only its physical mass to the admitted source, so settled ticks
    // do not allocate, sort identities, or reconstruct tensors.
    let source_count = q_wheels.iter().count() + q_contributions.iter().count();
    let admitted_count: usize = q_body
        .iter()
        .filter_map(|(_, _, _, _, _, _, _, _, _, _, state, _)| {
            state.map(|state| state.sources.len())
        })
        .sum();
    let inputs_changed = q_body.iter().any(
        |(
            _,
            mass,
            computed_mass,
            mass_override,
            inertia,
            inertia_override,
            center,
            computed_center,
            computed_inertia,
            center_override,
            state,
            _,
        )| {
            state.is_some_and(|state| {
                state.inputs
                    != ReducedComputedInputs::read(
                        mass,
                        computed_mass,
                        mass_override,
                        inertia,
                        computed_inertia,
                        inertia_override,
                        center,
                        computed_center,
                        center_override,
                    )
            })
        },
    );
    let keys_removed = removed_keys.read().count() != 0;
    let changed = source_count != admitted_count
        || inputs_changed
        || !changed_contributions.is_empty()
        || !changed_mounts.is_empty()
        || !changed_keys.is_empty()
        || keys_removed
        || q_wheels.iter().any(|(entity, wheel, mount, _)| {
            let Ok((_, _, _, _, _, _, _, _, _, _, Some(state), _)) = q_body.get(mount.body) else {
                return true;
            };
            let Some((_, source)) = state.sources.iter().find(|(source, _)| *source == entity)
            else {
                return true;
            };
            source.inv_mass != wheel.mass.recip() || source.local_com != mount.local.translation
        });
    if !changed {
        return;
    }

    let order = match lunco_physics::ordered_physics_entities(
        q_wheels
            .iter()
            .map(|(entity, _, _, key)| (entity, key))
            .chain(
                q_contributions
                    .iter()
                    .map(|(entity, contribution)| (entity, Some(&contribution.order_key))),
            ),
        "raycast wheel mass fold",
    ) {
        Ok(order) => order,
        Err(invalid) => {
            lunco_physics::report_invalid_physics_order(
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
                "physics-wheel-order-invalid",
                &invalid,
            );
            return;
        }
    };
    let mut carriers = Vec::new();
    let mut pending = HashSet::new();
    let mut additions_by_carrier = std::collections::HashMap::<
        Entity,
        Vec<(Entity, Result<MassProperties, &'static str>)>,
    >::new();
    for entity in order {
        let (owner, contribution) = if let Ok((_, wheel, mount, _)) = q_wheels.get(entity) {
            if wheel.mass == 0.0 {
                pending.insert(mount.body);
                continue;
            }
            (
                mount.body,
                checked_mass_properties(
                    mount.local.translation,
                    wheel.mass,
                    DVec3::ZERO,
                    DQuat::IDENTITY,
                ),
            )
        } else if let Ok((_, contribution)) = q_contributions.get(entity) {
            let properties = checked_mass_properties(
                contribution.center_of_mass,
                contribution.mass,
                contribution.principal,
                contribution.principal_axes,
            )
            .and_then(|properties| {
                if !contribution.local.translation.is_finite()
                    || !contribution.local.rotation.is_finite()
                    || !contribution.local.rotation.is_normalized()
                {
                    return Err("mass contribution has an invalid body-local mount");
                }
                Ok(properties.transform_by(&Pose::from_parts(
                    contribution.local.translation,
                    contribution.local.rotation.normalize(),
                )))
            });
            (contribution.owner, properties)
        } else {
            continue;
        };
        if !q_body.contains(owner) {
            continue;
        }
        if !additions_by_carrier.contains_key(&owner) {
            carriers.push(owner);
        }
        additions_by_carrier
            .entry(owner)
            .or_default()
            .push((entity, contribution));
    }
    // Sources can be deleted or reparented. Carriers with no remaining source
    // must still restore their independent baseline and native ownership.
    for (entity, _, _, _, _, _, _, _, _, _, state, _) in q_body.iter() {
        if state.is_some() && !carriers.contains(&entity) {
            carriers.push(entity);
        }
    }
    for carrier in carriers {
        if pending.contains(&carrier) {
            continue;
        }
        let Ok((
            _,
            mass,
            computed_mass,
            mass_authored,
            inertia,
            inertia_authored,
            com_override,
            com_computed,
            computed_inertia,
            center_authored,
            state,
            geometry_owner,
        )) = q_body.get_mut(carrier)
        else {
            continue;
        };
        let inputs = ReducedComputedInputs::read(
            mass.as_deref(),
            computed_mass,
            mass_authored,
            inertia.as_deref(),
            computed_inertia,
            inertia_authored,
            com_override,
            com_computed,
            center_authored,
        );
        let mass_authored = mass_authored && !geometry_owner.is_some_and(|owner| owner.mass);
        let inertia_authored =
            inertia_authored && !geometry_owner.is_some_and(|owner| owner.inertia);
        let center_authored = center_authored && !geometry_owner.is_some_and(|owner| owner.center);
        let sources = additions_by_carrier
            .remove(&carrier)
            .unwrap_or_default()
            .into_iter()
            .map(|(entity, properties)| properties.map(|properties| (entity, properties)))
            .collect::<Result<Vec<_>, _>>();
        if let (Some(state), Ok(sources)) = (state, &sources) {
            if state.sources == *sources && state.inputs == inputs {
                continue;
            }
        }
        let candidate = (|| {
            let sources = sources?;
            let baseline = if let Some(state) = state {
                state.baseline
            } else {
                let carrier_mass = if let Some(mass) = mass.as_deref().filter(|_| mass_authored) {
                    f64::from(mass.0)
                } else {
                    f64::from(
                        computed_mass
                            .filter(|mass| mass.is_finite())
                            .ok_or("carrier has no admitted mass baseline")?
                            .value(),
                    )
                };
                let center = com_override
                    .map(|c| c.0.as_dvec3())
                    .or_else(|| com_computed.map(|c| c.0.as_dvec3()))
                    .ok_or("carrier has no admitted centre-of-mass baseline")?;
                let (principal, frame) = if let Some(inertia) = inertia.as_deref() {
                    (inertia.principal.as_dvec3(), inertia.local_frame.as_dquat())
                } else {
                    lunco_physics::body_mass::computed_inertia_principal_d(
                        computed_inertia
                            .filter(|inertia| inertia.is_finite())
                            .ok_or("carrier has no admitted inertia baseline")?,
                    )?
                };
                if !frame.is_finite() || !frame.is_normalized() {
                    return Err("carrier has an invalid principal frame");
                }
                checked_mass_properties(center, carrier_mass, principal, frame.normalize())?
            };
            let mut total = lunco_physics::body_mass::sum_mass_properties(
                std::iter::once(baseline).chain(sources.iter().map(|(_, properties)| *properties)),
            )?;
            if let Some(mass) = inputs.mass {
                let automatic_inertia = state
                    .map(|state| state.automatic.1)
                    .unwrap_or(!inertia_authored);
                total.set_mass(
                    f64::from(mass.value()),
                    automatic_inertia && inputs.inertia.is_none(),
                );
            }
            let (principal, frame) = if let Some(inertia) = inputs.inertia {
                lunco_physics::body_mass::computed_inertia_principal_d(&inertia)?
            } else {
                (DVec3::ZERO, DQuat::IDENTITY)
            };
            let total = lunco_physics::body_mass::ColliderMassOverrides {
                principal: inputs.inertia.map(|_| principal),
                principal_axes: inputs.inertia.map(|_| frame),
                center: inputs.center.map(|center| center.0.as_dvec3()),
                ..default()
            }
            .apply(total)?;
            native_mass_properties(&total).map(|native| (native, baseline, sources))
        })();
        let ((new_mass, new_inertia, new_center), baseline, sources) = match candidate {
            Ok(candidate) => candidate,
            Err(detail) => {
                if let Some(holds) = holds.as_deref_mut() {
                    holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                }
                if let Some(time) = physics_time.as_deref_mut() {
                    use avian3d::schedule::PhysicsTime;
                    time.pause();
                    time.advance_by(std::time::Duration::ZERO);
                }
                if let Some(faults) = faults.as_deref_mut() {
                    faults.raise(
                        "physics-reduced-mass-invalid",
                        Some(carrier),
                        "reduced physical mass",
                        detail,
                    );
                }
                error!("refusing reduced mass candidate for {carrier:?}: {detail}");
                return;
            }
        };
        if inputs.mass.is_none() {
            if let Some(mut mass) = mass {
                *mass = new_mass;
            } else {
                commands.entity(carrier).try_insert(new_mass);
            }
        }
        if inputs.inertia.is_none() {
            if let Some(mut inertia) = inertia {
                *inertia = new_inertia;
            } else {
                commands.entity(carrier).try_insert(new_inertia);
            }
        }
        let automatic = state.map(|state| state.automatic).unwrap_or((
            !mass_authored,
            !inertia_authored,
            !center_authored,
        ));
        let original_geometry_owner = state
            .map(|state| state.geometry_owner)
            .unwrap_or(geometry_owner.copied());
        if inputs.center.is_none() {
            commands.entity(carrier).try_insert(new_center);
        }
        if sources.is_empty() {
            commands.entity(carrier).try_remove::<ReducedMassState>();
            if let Some(mut owner) = original_geometry_owner {
                owner.mass &= inputs.mass.is_none();
                owner.inertia &= inputs.inertia.is_none();
                owner.center &= inputs.center.is_none();
                commands.entity(carrier).try_insert(owner);
            } else {
                if automatic.0 && inputs.mass.is_none() {
                    commands.entity(carrier).try_remove::<(Mass, NoAutoMass)>();
                }
                if automatic.1 && inputs.inertia.is_none() {
                    commands
                        .entity(carrier)
                        .try_remove::<(AngularInertia, NoAutoAngularInertia)>();
                }
                if automatic.2 && inputs.center.is_none() {
                    commands
                        .entity(carrier)
                        .try_remove::<(CenterOfMass, NoAutoCenterOfMass)>();
                }
            }
        } else {
            if geometry_owner.is_some() {
                commands
                    .entity(carrier)
                    .try_insert(lunco_physics::body_mass::BodyMassFromColliders::default());
            }
            commands.entity(carrier).try_insert((
                NoAutoMass,
                NoAutoCenterOfMass,
                NoAutoAngularInertia,
                ReducedMassState {
                    baseline,
                    sources,
                    automatic,
                    geometry_owner: original_geometry_owner,
                    inputs,
                },
            ));
        }
    }
}

/// How far ABOVE the axle a raycast wheel's suspension ray starts — the STRUT TOP.
///
/// THE WHEEL PRIM IS THE AXLE, in both realizations. A raycast strut hangs the hub
/// `rest_length` below its cast origin and the tire holds the hub `wheel_radius`
/// above the ground, so at rest the strut occupies exactly `rest_length −
/// wheel_radius` and its top sits that far above the authored mount. Casting from
/// there puts the hub AT the authored mount at rest, which is where the physical
/// realization's wheel body actually is.
///
/// This used to be baked into the asset instead: `raycast_drivetrain.usda` authored
/// the wheel prims 0.5 m higher than `physical_drivetrain.usda` so the raycast rover
/// would end up at a plausible ride height. One rover then had two mount heights and
/// two centre-of-mass heights depending on a variant switch — and 0.5 m was not even
/// the 0.3 m the authored spring implies. Deriving it here means a suspension swap
/// (`rocker.usda`, `rigid.usda`) moves the strut with it, and nothing needs re-typing.
///
/// A `rigid` mount (`rest_length` 0) returns `−wheel_radius`: the ray starts at the
/// contact patch, which is where a wheel bolted straight to the hull touches down.
pub fn strut_offset(rest_length: f64, wheel_radius: f64) -> f64 {
    rest_length - wheel_radius
}

/// Return the query length for a suspension ray whose physical travel is
/// `rest_length`.
///
/// Avian accepts a ray hit only when `distance < max_distance`. A freshly
/// settled wheel is intentionally placed at exactly its authored rest length,
/// so using `rest_length` as the query bound makes that valid contact
/// disappear because of a strict boundary comparison (and because authored
/// USD values have already crossed the f32/f64 boundary once). The added
/// micrometre-scale margin is numerical query tolerance only: suspension
/// compression and force still use the authored `rest_length`, so it cannot
/// create physical travel or load.
pub fn suspension_ray_max_distance(rest_length: f64) -> f64 {
    if !rest_length.is_finite() || rest_length <= 0.0 {
        return rest_length;
    }
    rest_length + 1.0e-6 * rest_length.max(1.0)
}

/// Resolve the authored ray's ground contact in the same physics frame as the
/// wheel hub.
///
/// The ray starts at the strut top and points along the wheel's local -Y axis.
/// The returned point is the point at which the ground reaction acts. Both the
/// analytical suspension force and the analytical tire force must use this
/// point so the reduced realization preserves the contact lever arm and load
/// transfer of the physical wheel realization. The wheel entity is the axle
/// and the authored ray starts at the strut top. Presentation consumers use
/// this same reconstruction for contact-derived annotations rather than
/// rebuilding a second wheel geometry model.
///
/// This is the solved ground-contact point for a raycast wheel.
#[inline]
pub fn raycast_contact_point(
    hub_position: DVec3,
    hub_rotation: DQuat,
    rest_length: f64,
    wheel_radius: f64,
    hit_distance: f64,
) -> DVec3 {
    let ray_origin =
        hub_position + hub_rotation * DVec3::Y * strut_offset(rest_length, wheel_radius);
    ray_origin + hub_rotation * DVec3::NEG_Y * hit_distance
}

/// Upper clamp on the suspension force magnitude (N) applied per spring.
/// Bounds the spring+damping sum so a deeply-compressed strut or a numerical
/// velocity spike can't inject an explosive impulse that launches the rover.
const MAX_SUSPENSION_FORCE_N: f64 = 100_000.0;

// ── Pure force laws (unit-tested; the numerically-sensitive bits live here) ─────

/// Contact friction opposing the slip *velocity vector*. Continuous through zero
/// (no dead-band) so a near-stationary wheel is still damped — a slip dead-band
/// left sub-threshold motion undamped and produced a stiction limit-cycle (the
/// steering jitter). Linear `-k·slip` below the Coulomb cone, saturating at it.
/// While `braking`, a locked wheel grips at the FULL cone (opposing all sliding)
/// so it actually decelerates the chassis.
/// Wheel traction basis projected into the contact plane defined by `normal`.
///
/// Returns `(forward, right)`: orthonormal vectors spanning the plane ⟂ to the
/// contact `normal`, with `forward` the wheel heading projected into that plane
/// and `right = forward × normal`. For an upright wheel (where `normal` is the
/// wheel's own up, so the heading already lies in the plane) this reproduces the
/// raw `(wheel_forward, wheel_right)` — existing rovers are byte-for-byte
/// unchanged. For a **leaning single-track vehicle** the contact normal tilts
/// with the lean, so decomposing slip/drive in this basis gives the correct
/// longitudinal/lateral split instead of assuming a flat patch. Falls back to
/// the raw vectors if the heading is parallel to the normal (degenerate).
pub fn contact_plane_basis(
    wheel_forward: DVec3,
    wheel_right: DVec3,
    normal: DVec3,
) -> (DVec3, DVec3) {
    let n = normal.normalize_or_zero();
    if n == DVec3::ZERO {
        return (wheel_forward, wheel_right);
    }
    let forward = (wheel_forward - n * wheel_forward.dot(n))
        .try_normalize()
        .unwrap_or(wheel_forward);
    let right = forward.cross(n).try_normalize().unwrap_or(wheel_right);
    (forward, right)
}

/// Collider-backed wheel traction follows its signed physical axle. Projecting
/// the chassis heading instead reverses traction as the chassis pitches through
/// vertical, although the axle and positive motor coordinate stay unchanged.
/// Retain the heading basis for degenerate sidewall contacts, where axle and
/// normal are parallel and no circumferential rolling tangent is defined.
pub fn jointed_contact_plane_basis(
    wheel_forward: DVec3,
    axle: DVec3,
    coordinate_sign: f64,
    normal: DVec3,
) -> (DVec3, DVec3) {
    let n = normal.normalize_or_zero();
    let Some(forward) = (coordinate_sign * axle.cross(n)).try_normalize() else {
        return contact_plane_basis(wheel_forward, axle, normal);
    };
    (forward, forward.cross(n).normalize_or_zero())
}

/// Whether a solved contact carries a physical normal load. Tire, drive and
/// parking consumers share this scale-independent boundary; a sub-newton load
/// remains a contact and contributes its proportionate friction capacity.
pub(crate) fn contact_has_positive_load(normal_force: f64) -> bool {
    normal_force.is_finite() && normal_force > 0.0
}

/// The standard PhysX tire lateral-stiffness graph projected into the shared
/// LunCo tire law.
///
/// USD authors `(minimum normalized load, maximum lateral stiffness)` through
/// `physxVehicleTire:lateralStiffnessGraph` and the reference load through
/// `physxVehicleTire:restLoad`. The graph is linear from zero load to its
/// plateau, exactly as the PhysX schema defines it. `max_stiffness` is an
/// absolute force-per-radian value; it is not a LunCo-specific normalized
/// coefficient.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct TireLateralStiffnessGraph {
    /// Normalized tire load at which the graph reaches its plateau.
    pub minimum_normalized_load: f64,
    /// Plateau lateral stiffness, N/rad.
    pub max_stiffness: f64,
    /// Authored rest load used to normalize the current contact load, N.
    pub rest_load: f64,
}

impl TireLateralStiffnessGraph {
    /// Evaluate the standard graph at one current normal load.
    pub fn stiffness_at_load(self, normal_force: f64) -> f64 {
        if !(normal_force.is_finite()
            && normal_force > 0.0
            && self.max_stiffness.is_finite()
            && self.max_stiffness > 0.0
            && self.rest_load.is_finite()
            && self.rest_load > 0.0
            && self.minimum_normalized_load.is_finite()
            && self.minimum_normalized_load >= 0.0)
        {
            return 0.0;
        }
        let scale = if self.minimum_normalized_load > 0.0 {
            (normal_force / self.rest_load / self.minimum_normalized_load).min(1.0)
        } else {
            1.0
        };
        self.max_stiffness * scale
    }
}

/// Standard PhysX minimum longitudinal slip speeds, in canonical m/s.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct LongitudinalSlipDenominators {
    pub active_mps: f64,
    pub passive_mps: f64,
}

impl Default for LongitudinalSlipDenominators {
    fn default() -> Self {
        Self {
            active_mps: 0.1,
            passive_mps: 4.0,
        }
    }
}

impl LongitudinalSlipDenominators {
    pub fn minimum(self, active: bool) -> f64 {
        if active {
            self.active_mps
        } else {
            self.passive_mps
        }
    }
}

/// Resolve longitudinal and lateral slip into one Coulomb-limited
/// contact-patch force.
///
/// `longitudinal_stiffness` is force per dimensionless slip. Longitudinal slip
/// is `longitudinal_slip_speed / longitudinal_reference_speed`. The vehicle's
/// positive active/passive denominator bounds this longitudinal reference;
/// it is independent of the tire's validated lateral-speed envelope.
/// `lateral_stiffness_graph` is evaluated at this contact's normal load using
/// the standard PhysX graph. `rolling_reference_speed` is derived by the wheel
/// realization as `max(|v_hub,long|, |omega*r|, min_validated_speed)`: the actual
/// longitudinal motion and the authored tire's validated speed boundary,
/// including a counter-rotating pivot wheel. If translation and rotation are both
/// zero while the patch moves sideways, the angle tends to 90 degrees and the
/// ordinary Coulomb limit supplies static lateral resistance.
///
/// The final pair is scaled onto the single Coulomb cone without changing its
/// direction. Both wheel realizations call this function: raycast wheels apply
/// its result to the chassis, while jointed wheels apply it to the wheel body at
/// Avian's saved contact point. The Avian bridge retains the normal constraint
/// but disables its generic tangent impulse for jointed tire contacts, so this
/// remains the only tire model.
pub fn tire_patch_force(
    longitudinal_stiffness: f64,
    longitudinal_slip_speed: f64,
    longitudinal_reference_speed: f64,
    rolling_reference_speed: f64,
    lateral_speed: f64,
    normal_force: f64,
    friction_mu: f64,
    lateral_stiffness_graph: TireLateralStiffnessGraph,
) -> (f64, f64) {
    if normal_force <= 0.0 {
        return (0.0, 0.0);
    }

    let longitudinal_force =
        longitudinal_stiffness * (longitudinal_slip_speed / longitudinal_reference_speed);

    let lateral_force = if lateral_stiffness_graph.max_stiffness > 0.0 {
        let slip_angle = (-lateral_speed).atan2(rolling_reference_speed.abs());
        lateral_stiffness_graph.stiffness_at_load(normal_force) * slip_angle
    } else {
        0.0
    };

    let cone = friction_mu.max(0.0) * normal_force;
    let magnitude = longitudinal_force.hypot(lateral_force);
    if magnitude > cone {
        let scale = cone / magnitude;
        (longitudinal_force * scale, lateral_force * scale)
    } else {
        (longitudinal_force, lateral_force)
    }
}

/// Resolve the static-friction force for a locked wheel, including the
/// measured tangential contact velocity.
///
/// The caller apportions actual chassis mass by measured contact normal load.
/// Suction or aerodynamic preload must not be mistaken for additional mass,
/// and contacts may be vertical. `correction_mass` is the mass whose velocity
/// the contact force directly corrects. The fixed-step request is capped by
/// the authored friction cone; an under-gripped surface still slides.
fn parking_brake_force_with_velocity(
    gravity: DVec3,
    contact_normal: DVec3,
    normal_force: f64,
    supported_mass: f64,
    correction_mass: f64,
    friction_mu: f64,
    tangential_velocity: DVec3,
    dt: f64,
) -> DVec3 {
    if dt <= 0.0 || !dt.is_finite() || !tangential_velocity.is_finite() {
        return DVec3::ZERO;
    }
    let normal = contact_normal.normalize_or_zero();
    if normal == DVec3::ZERO {
        return DVec3::ZERO;
    }
    if normal_force <= 0.0
        || friction_mu <= 0.0
        || supported_mass <= 0.0
        || !supported_mass.is_finite()
        || correction_mass <= 0.0
        || !correction_mass.is_finite()
    {
        return DVec3::ZERO;
    }

    let tangent_gravity = gravity - normal * gravity.dot(normal);
    let tangent_velocity = tangential_velocity - normal * tangential_velocity.dot(normal);
    let mut requested =
        -tangent_gravity * supported_mass - tangent_velocity * (correction_mass / dt);
    // A sliding contact cannot propel its own slip. Static friction may hold
    // against gravity at zero velocity, where it performs no work.
    let slip_squared = tangent_velocity.length_squared();
    if slip_squared > 0.0 {
        let propelling_work = requested.dot(tangent_velocity);
        if propelling_work > 0.0 {
            requested -= tangent_velocity * (propelling_work / slip_squared);
        }
    }
    let available = friction_mu * normal_force;
    let magnitude = requested.length();
    if magnitude == 0.0 {
        DVec3::ZERO
    } else {
        requested * (available / magnitude).min(1.0)
    }
}

/// A mechanical interval ending at the implicit contact-force stage. Splitting
/// at a brake stop prevents reversal chatter and retains the finite braking work.
#[derive(Clone, Copy, Debug, Default)]
pub struct AxleInterval {
    pub start_speed: f64,
    pub speed: f64,
    pub brake_torque: f64,
    pub duration: f64,
}

/// One axle step contains at most one zero-speed crossing for the monotone tire
/// law and constant drive command. No allocation or independent thermal state.
#[derive(Clone, Copy, Debug)]
pub struct AxleStep {
    pub speed: f64,
    pub intervals: [AxleInterval; 2],
}

/// Backward-Euler axle balance with viscous drag and passive Coulomb braking.
/// `traction_torque(speed)` includes the final combined friction cone. The
/// bound is the sum of each contact's `radius * mu * load`. Brake stopping is
/// resolved inside the interval; thermal work uses its angular displacement,
/// rather than the zero terminal speed. Contact/bearing rates use the implicit
/// stage. Numerical integration work is separate from physical friction heat.
pub fn solve_axle_step(
    speed: f64,
    inertia: f64,
    bearing_damping: f64,
    drive_torque: f64,
    brake_limit: f64,
    traction_bound: f64,
    dt: f64,
    traction_torque: impl Fn(f64) -> f64,
) -> Result<AxleStep, &'static str> {
    solve_axle_step_with_roundoff(
        speed,
        inertia,
        bearing_damping,
        drive_torque,
        brake_limit,
        traction_bound,
        dt,
        traction_torque,
        |_| 0.0,
    )
}

/// The native tire additionally certifies its floating-point evaluation error.
/// This bound is consulted only when the root bracket reaches adjacent f64
/// speeds, never as a relaxed ordinary convergence target.
pub(crate) fn solve_axle_step_with_roundoff(
    speed: f64,
    inertia: f64,
    bearing_damping: f64,
    drive_torque: f64,
    brake_limit: f64,
    traction_bound: f64,
    dt: f64,
    traction_torque: impl Fn(f64) -> f64,
    contact_roundoff: impl Fn(f64) -> f64,
) -> Result<AxleStep, &'static str> {
    if ![
        speed,
        inertia,
        bearing_damping,
        drive_torque,
        brake_limit,
        traction_bound,
        dt,
    ]
    .into_iter()
    .all(f64::is_finite)
        || inertia <= 0.0
        || dt <= 0.0
        || bearing_damping < 0.0
        || brake_limit < 0.0
        || traction_bound < 0.0
    {
        return Err(
            "axle requires finite state, positive inertia/time and nonnegative passive coefficients",
        );
    }
    let rest_torque = drive_torque - traction_torque(0.0);
    if !rest_torque.is_finite() {
        return Err("axle contact torque is not finite");
    }
    let advance = |old: f64, h: f64, brake: f64| -> Result<f64, &'static str> {
        let rhs = inertia / h * old + drive_torque + brake;
        let diagonal = inertia / h + bearing_damping;
        let mut lower = (rhs - traction_bound) / diagonal;
        let mut upper = (rhs + traction_bound) / diagonal;
        if ![lower, upper].into_iter().all(f64::is_finite) {
            return Err("axle step is not representable");
        }
        for _ in 0..128 {
            let middle = lower + (upper - lower) * 0.5;
            let contact = traction_torque(middle);
            let inertial = diagonal * middle;
            let residual = inertial + contact - rhs;
            if !residual.is_finite() {
                return Err("axle contact residual is not finite");
            }
            let scale = inertial.abs().max(contact.abs()).max(rhs.abs());
            if residual == 0.0 || residual.abs() <= scale * (64.0 * f64::EPSILON) {
                return Ok(middle);
            }
            if middle == lower || middle == upper {
                // At adjacent representable speeds no further refinement is
                // possible. Slip subtracts nearly equal rolling/contact speeds;
                // certify that evaluation error against the admitted contact
                // torque range, while retaining the tighter ordinary target.
                let evaluation_error = contact_roundoff(middle);
                if !evaluation_error.is_finite() || evaluation_error < 0.0 {
                    return Err("axle contact evaluation error is invalid");
                }
                if residual.abs()
                    <= (scale.max(traction_bound) * (64.0 * f64::EPSILON)).max(evaluation_error)
                {
                    return Ok(middle);
                }
                break;
            }
            if residual > 0.0 {
                upper = middle;
            } else {
                lower = middle;
            }
        }
        Err("axle contact law did not converge to a finite torque balance")
    };
    let from_rest = |h: f64| -> Result<AxleInterval, &'static str> {
        if rest_torque.abs() <= brake_limit {
            return Ok(AxleInterval {
                start_speed: 0.0,
                speed: 0.0,
                brake_torque: -rest_torque,
                duration: h,
            });
        }
        let brake = -brake_limit * rest_torque.signum();
        Ok(AxleInterval {
            start_speed: 0.0,
            speed: advance(0.0, h, brake)?,
            brake_torque: brake,
            duration: h,
        })
    };
    if speed == 0.0 {
        let interval = from_rest(dt)?;
        return Ok(AxleStep {
            speed: interval.speed,
            intervals: [interval, AxleInterval::default()],
        });
    }
    let brake = -brake_limit * speed.signum();
    let candidate = advance(speed, dt, brake)?;
    if candidate * speed >= 0.0 || brake_limit == 0.0 {
        return Ok(AxleStep {
            speed: candidate,
            intervals: [
                AxleInterval {
                    start_speed: speed,
                    speed: candidate,
                    brake_torque: brake,
                    duration: dt,
                },
                AxleInterval::default(),
            ],
        });
    }
    // The implicit endpoint at zero has no viscous torque. The constant
    // command/contact torque determines the stop time, before any reversal.
    let stop_time = -inertia * speed / (rest_torque + brake);
    if !stop_time.is_finite() || stop_time <= 0.0 || stop_time > dt * (1.0 + 64.0 * f64::EPSILON) {
        return Err("axle brake stop could not be resolved within the step");
    }
    let stop_time = stop_time.min(dt);
    let first = AxleInterval {
        start_speed: speed,
        speed: 0.0,
        brake_torque: brake,
        duration: stop_time,
    };
    let second = if stop_time < dt {
        from_rest(dt - stop_time)?
    } else {
        AxleInterval::default()
    };
    Ok(AxleStep {
        speed: second.speed,
        intervals: [first, second],
    })
}

#[cfg(test)]
mod tire_patch_tests {
    use super::{TireLateralStiffnessGraph, solve_axle_step, tire_patch_force};
    use bevy::math::DVec3;

    #[test]
    fn axle_starts_both_directions_with_standard_slip_denominators_at_tiny_and_unit_force() {
        let minimum = super::LongitudinalSlipDenominators::default().minimum(true);
        for scale in [1.0, 1e-24] {
            for drive in [-10.0, -1.0, 1.0, 10.0] {
                let drive = drive * scale;
                let patch = |speed: f64| {
                    tire_patch_force(
                        14_000.0 * scale,
                        speed * 0.4,
                        (speed * 0.4).abs().max(minimum),
                        0.0,
                        0.0,
                        400.0 * scale,
                        1.1,
                        TireLateralStiffnessGraph::default(),
                    )
                    .0
                };
                let step = solve_axle_step(
                    0.0,
                    2.0 * scale,
                    0.45 * scale,
                    drive,
                    0.0,
                    176.0 * scale,
                    1.0 / 60.0,
                    |speed| patch(speed) * 0.4,
                )
                .unwrap();
                assert!(step.speed * drive > 0.0);
                let residual =
                    (120.0 + 0.45) * scale * step.speed + patch(step.speed) * 0.4 - drive;
                assert!(residual.abs() < drive.abs() * 1e-12);
            }
        }
    }

    #[test]
    fn axle_balances_near_rolling_contact_at_representable_speed() {
        for scale in [1.0, 1e-24] {
            let radius = 0.465;
            let velocity: f64 = -0.03156994047678835;
            let initial = -0.034873556114436405;
            let drive = 3.53403831590483 * scale;
            let bound = radius * 0.6 * 400.0 * scale;
            let contact = |speed: f64| {
                tire_patch_force(
                    14_000.0 * scale,
                    speed * radius - velocity,
                    velocity.abs().max((speed * radius).abs()).max(0.1),
                    0.0,
                    0.0,
                    400.0 * scale,
                    0.6,
                    TireLateralStiffnessGraph::default(),
                )
                .0 * radius
            };
            let step = solve_axle_step(
                initial,
                1.946 * scale,
                0.45 * scale,
                drive,
                0.0,
                bound,
                1.0 / 60.0,
                contact,
            )
            .unwrap();
            let residual = (1.946 * 60.0 + 0.45) * scale * step.speed + contact(step.speed)
                - (1.946 * 60.0 * scale * initial + drive);
            assert!(residual.abs() <= bound * (64.0 * f64::EPSILON));
            assert!((step.speed - -0.0676481799699605).abs() < 1e-12);
        }
    }

    #[test]
    fn axle_rejects_a_discontinuous_contact_law_without_a_torque_balance() {
        for offset in [0.0, 1.0] {
            let result =
                solve_axle_step(offset, 2.0, 0.45, 10.0, 0.0, 176.0, 1.0 / 60.0, |speed| {
                    if speed == offset {
                        0.0
                    } else {
                        176.0 * (speed - offset).signum()
                    }
                });
            assert_eq!(
                result.unwrap_err(),
                "axle contact law did not converge to a finite torque balance"
            );
        }
    }

    #[test]
    fn axle_resolves_stiff_rolling_contact_with_rotating_housing_at_float_boundary() {
        for scale in [1.0, 1e-24] {
            let radius = 0.465;
            let housing = 0.01_f64;
            let velocity = 0.05_f64;
            let initial = velocity / radius - housing;
            let bound = radius * 0.6 * scale;
            let contact = |speed: f64| {
                let tread = (speed + housing) * radius;
                tire_patch_force(
                    14_000.0 * scale,
                    tread - velocity,
                    tread.abs().max(velocity.abs()).max(0.1),
                    0.0,
                    0.0,
                    scale,
                    0.6,
                    TireLateralStiffnessGraph::default(),
                )
                .0 * radius
            };
            let error = |speed: f64| {
                64.0 * f64::EPSILON
                    * radius
                    * 14_000.0
                    * scale
                    * (radius * (speed.abs() + housing.abs()) + velocity.abs())
                    / 0.1
            };
            let result = super::solve_axle_step_with_roundoff(
                initial,
                1.946025 * scale,
                0.45 * scale,
                0.1 * scale,
                0.0,
                bound,
                1.0 / 60.0,
                contact,
                error,
            )
            .unwrap();
            let residual = (1.946025 * 60.0 + 0.45) * scale * result.speed + contact(result.speed)
                - (1.946025 * 60.0 * scale * initial + 0.1 * scale);
            assert!(residual.abs() <= error(result.speed));
            assert!((result.speed - 0.09752872822528194).abs() < 1e-14);
        }
    }

    #[test]
    fn longitudinal_stiffness_uses_dimensionless_slip() {
        let graph = TireLateralStiffnessGraph::default();
        for speed_scale in [0.01, 1.0, 100.0] {
            let (forward, _) = tire_patch_force(
                1000.0,
                0.5 * speed_scale,
                10.0 * speed_scale,
                10.0 * speed_scale,
                0.0,
                1000.0,
                1.0,
                graph,
            );
            let (reverse, _) = tire_patch_force(
                1000.0,
                -0.5 * speed_scale,
                10.0 * speed_scale,
                10.0 * speed_scale,
                0.0,
                1000.0,
                1.0,
                graph,
            );
            assert!((forward - 50.0).abs() < 1e-12);
            assert!((reverse + 50.0).abs() < 1e-12);
        }
        assert_eq!(
            tire_patch_force(1000.0, 0.0, 0.1, 0.0, 0.0, 1000.0, 1.0, graph),
            (0.0, 0.0)
        );
    }

    #[test]
    fn rolling_speed_defines_pivot_slip_angle_without_a_threshold() {
        let graph = TireLateralStiffnessGraph {
            minimum_normalized_load: 1.0,
            max_stiffness: 2.9 * 400.0,
            rest_load: 400.0,
        };
        let (_, lateral) = tire_patch_force(0.0, 0.0, 2.0, 2.0, 1.0, 400.0, 10.0, graph);
        let expected = 2.9 * 400.0 * (-0.5_f64).atan();
        assert!((lateral - expected).abs() < 1.0e-9);
    }

    #[test]
    fn combined_force_preserves_direction_at_coulomb_limit() {
        for scale in [1.0, 1e-24] {
            let graph = TireLateralStiffnessGraph {
                minimum_normalized_load: 1.0,
                max_stiffness: 2.9 * 200.0 * scale,
                rest_load: 200.0 * scale,
            };
            let (long, lateral) = tire_patch_force(
                600.0 * scale,
                2.0,
                2.0,
                2.0,
                -1.0,
                200.0 * scale,
                1.5,
                graph,
            );
            assert!((long.hypot(lateral) - 300.0 * scale).abs() < 1e-9 * scale);
            assert!(long > 0.0 && lateral > 0.0);
        }
    }

    #[test]
    fn zero_load_has_no_patch_force() {
        assert_eq!(
            tire_patch_force(
                10.0,
                1.0,
                1.0,
                1.0,
                1.0,
                0.0,
                1.0,
                TireLateralStiffnessGraph {
                    minimum_normalized_load: 1.0,
                    max_stiffness: 1_160.0,
                    rest_load: 400.0,
                },
            ),
            (0.0, 0.0)
        );
    }

    #[test]
    fn lateral_graph_scales_stiffness_below_its_plateau_load() {
        let graph = TireLateralStiffnessGraph {
            minimum_normalized_load: 2.0,
            max_stiffness: 1_000.0,
            rest_load: 400.0,
        };
        assert_eq!(graph.stiffness_at_load(200.0), 250.0);
        assert_eq!(graph.stiffness_at_load(800.0), 1_000.0);
    }

    #[test]
    fn parking_brake_cancels_gravity_on_a_holdable_slope() {
        let gravity = DVec3::new(0.0, -1.62, -0.8);
        let normal_force = 400.0;
        let force = super::parking_brake_force_with_velocity(
            gravity,
            DVec3::Y,
            normal_force,
            normal_force / 1.62,
            normal_force / 1.62,
            1.5,
            DVec3::ZERO,
            1.0 / 60.0,
        );
        assert!(force.z > 0.0);
        assert!((force.y).abs() < 1.0e-12);
        assert!(force.length() <= 1.5 * normal_force + 1.0e-9);
        let supported_mass = normal_force / 1.62;
        assert!(
            (force.z + supported_mass * gravity.z).abs() < 1.0e-9,
            "parking force must exactly balance this contact's tangential gravity share"
        );
    }

    #[test]
    fn parking_brake_holds_a_preloaded_vertical_contact() {
        for scale in [1.0, 1e-24] {
            let force = super::parking_brake_force_with_velocity(
                DVec3::new(0.0, -9.80665, 0.0),
                DVec3::Z,
                10000.0 * scale,
                487.5 * scale,
                487.5 * scale,
                1.4,
                DVec3::ZERO,
                1.0 / 60.0,
            );
            assert!(
                (force.y - 9.80665 * 487.5 * scale).abs() < 1e-9 * scale,
                "a vertical contact must resist tangential gravity at its physical force scale"
            );
        }
    }

    #[test]
    fn parking_preload_does_not_inflate_inertial_mass() {
        let force = super::parking_brake_force_with_velocity(
            DVec3::new(0.0, -9.0, -1.0),
            DVec3::Y,
            10000.0,
            100.0,
            100.0,
            1.4,
            DVec3::ZERO,
            1.0 / 60.0,
        );
        assert!((force.z - 100.0).abs() < 1.0e-9);
        let limited = super::parking_brake_force_with_velocity(
            DVec3::new(0.0, -9.80665, 0.0),
            DVec3::Z,
            1000.0,
            487.5,
            487.5,
            0.2,
            DVec3::ZERO,
            1.0 / 60.0,
        );
        assert!(
            (limited.y - 200.0).abs() < 1.0e-9,
            "insufficient adhesion must remain friction limited"
        );
    }

    #[test]
    fn parking_brake_opposes_residual_contact_velocity() {
        let gravity = DVec3::new(0.0, -1.62, 0.0);
        let force = super::parking_brake_force_with_velocity(
            gravity,
            DVec3::Y,
            400.0,
            400.0 / 1.62,
            400.0 / 1.62,
            1.5,
            DVec3::new(2.0, 0.0, -1.0),
            0.1,
        );
        assert!(force.x < 0.0 && force.z > 0.0);
        assert!(force.length() <= 1.5 * 400.0 + 1.0e-9);
    }

    #[test]
    fn parking_contact_uses_wheel_mass_for_velocity_and_load_share_for_gravity() {
        let force = super::parking_brake_force_with_velocity(
            DVec3::new(0.0, -1.0, -0.2),
            DVec3::Y,
            1_000.0,
            100.0,
            25.0,
            1.0,
            DVec3::new(0.1, 0.0, 0.0),
            0.1,
        );
        assert!((force.x + 25.0).abs() < 1e-12);
        assert!((force.z - 20.0).abs() < 1e-12);
    }

    #[test]
    fn axle_step_conserves_work_and_keeps_physical_heat_passive() {
        let graph = TireLateralStiffnessGraph {
            minimum_normalized_load: 1.0,
            max_stiffness: 1200.0,
            rest_load: 400.0,
        };
        let radius = 0.4;
        let inertia = 6.8;
        let dt = 1.0 / 60.0;
        let damping = 0.45;
        for old in [-5.0, 0.0, 5.0] {
            for drive in [-400.0, 0.0, 400.0] {
                for hub in [-2.0, 0.0, 2.0] {
                    for lateral in [-1.0, 0.0, 1.0] {
                        for load in [0.0, 455.0] {
                            for brake in [0.0, 300.0] {
                                let patch = |w: f64| {
                                    tire_patch_force(
                                        8000.0,
                                        w * radius - hub,
                                        hub.abs().max((w * radius).abs()).max(0.1),
                                        hub.abs().max((w * radius).abs()).max(0.1),
                                        lateral,
                                        load,
                                        1.5,
                                        graph,
                                    )
                                };
                                let step = solve_axle_step(
                                    old,
                                    inertia,
                                    damping,
                                    drive,
                                    brake,
                                    radius * 1.5 * load,
                                    dt,
                                    |w| patch(w).0 * radius,
                                )
                                .unwrap();
                                for interval in
                                    step.intervals.into_iter().filter(|s| s.duration > 0.0)
                                {
                                    let w = interval.speed;
                                    let old = interval.start_speed;
                                    let dt = interval.duration;
                                    let (long, lat) = patch(w);
                                    let bearing_heat = damping * w * w;
                                    let brake_heat = -interval.brake_torque * (old + w) * 0.5;
                                    let tire_heat = long * (w * radius - hub) - lat * lateral;
                                    assert!(
                                        bearing_heat >= 0.0
                                            && brake_heat >= -1.0e-8
                                            && tire_heat >= -1.0e-8
                                    );
                                    assert!(long.hypot(lat) <= 1.5 * load + 1.0e-8);
                                    let stored = inertia * (w * w - old * old) / (2.0 * dt);
                                    let numerical = inertia * (w - old).powi(2) / (2.0 * dt)
                                        + interval.brake_torque * (old - w) * 0.5;
                                    let balance = drive * w
                                        - long * hub
                                        - lat * lateral
                                        - bearing_heat
                                        - brake_heat
                                        - tire_heat
                                        - stored
                                        - numerical;
                                    assert!(
                                        balance.abs() < 1.0e-6,
                                        "discrete work residual {balance}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn axle_brake_holds_and_airborne_bearing_drag_cannot_reverse_the_shaft() {
        let stopped = solve_axle_step(0.0, 2.0, 0.45, 200.0, 300.0, 0.0, 0.1, |_| 0.0).unwrap();
        assert_eq!(stopped.speed, 0.0);
        assert_eq!(stopped.intervals[0].brake_torque, -200.0);
        let damped = solve_axle_step(1.0, 2.0, 2000.0, 0.0, 0.0, 0.0, 0.1, |_| 0.0).unwrap();
        assert!(damped.speed >= 0.0 && damped.speed < 1.0);
        assert!(solve_axle_step(1.0, 2.0, -0.45, 0.0, 0.0, 0.0, 0.1, |_| 0.0).is_err());
    }

    #[test]
    fn a_brake_stop_retains_kinetic_heat_but_static_holding_has_no_work() {
        let step = solve_axle_step(1.0, 2.0, 0.0, 0.0, 100.0, 0.0, 0.1, |_| 0.0).unwrap();
        assert_eq!(step.speed, 0.0);
        let energy = step
            .intervals
            .iter()
            .map(|s| -s.brake_torque * (s.start_speed + s.speed) * 0.5 * s.duration)
            .sum::<f64>();
        assert!((energy - 1.0).abs() < 1.0e-12);
        let holding = solve_axle_step(0.0, 2.0, 0.0, 50.0, 100.0, 0.0, 0.1, |_| 0.0).unwrap();
        assert_eq!(holding.speed, 0.0);
        assert_eq!(holding.intervals[0].start_speed, 0.0);
    }

    #[test]
    fn a_parking_contact_never_propels_an_existing_slip() {
        let velocity = DVec3::new(0.0, 0.0, 0.001);
        let force = super::parking_brake_force_with_velocity(
            DVec3::new(0.0, -1.62, -0.8),
            DVec3::Y,
            400.0,
            400.0 / 1.62,
            400.0 / 1.62,
            1.5,
            velocity,
            1.0 / 60.0,
        );
        assert!(force.dot(velocity) <= 1.0e-12);
    }

    #[test]
    fn saturated_drive_uses_current_candidate_after_a_small_residual_reversal() {
        let traction = |w: f64| (8_000.0 * (w * 0.4)).clamp(-1.5 * 455.0, 1.5 * 455.0);
        let step = solve_axle_step(
            -1.0e-3,
            6.8,
            0.0,
            400.0,
            0.0,
            0.4 * 1.5 * 455.0,
            1.0 / 60.0,
            |w| traction(w) * 0.4,
        )
        .unwrap();
        let force = traction(step.speed);
        assert!(force > 0.0);
    }
}

/// Suspension normal-force magnitude: spring `k·x` plus damping `c·v`, with the
/// DAMPING bounded to ±spring so the total stays in `[0, 2·spring]` without a
/// `.max(0)` cliff. The cliff (clamping the *total* to ≥0) dropped damping on the
/// rebound half-cycle → an undamped suspension limit-cycle (the forward+turn
/// jitter); unbounded `c·v` also spiked the force on hard hits. Bounding the
/// damping term fixes both. The total is capped at [`MAX_SUSPENSION_FORCE_N`],
/// so a wheel spawned intersecting terrain can't launch the rover.
fn suspension_force_mag(compression: f64, spring_k: f64, relative_vel: f64, damping_c: f64) -> f64 {
    let spring = compression * spring_k;
    let damping = (relative_vel * damping_c).clamp(-spring, spring);
    (spring + damping).clamp(0.0, MAX_SUSPENSION_FORCE_N)
}

/// equation, simulating the behavior of a physical tire and strut.
#[derive(Component, Debug, Default)]
struct WheelRaycastParallelState {
    enabled_before_native_cast: bool,
}

#[derive(Component, Debug, Clone, Reflect)]
#[require(WheelRaycastParallelState)]
#[reflect(Component, Default)]
pub struct WheelRaycast {
    /// Port mapping for suspension telemetry.
    pub suspension_port: Entity,
    /// Port mapping for drive torque actuation.
    pub drive_port: Entity,
    /// Port mapping for the measured shaft speed fed back to the solved network.
    pub speed_port: Entity,
    /// Port mapping for the final wheel heading command.
    pub heading_port: Entity,
    /// Radius of the tire (effectively the minimum offset from ground).
    pub wheel_radius: f64,
    /// Full authored tire width used by generic presentation consumers.
    pub wheel_width: f64,
    /// Entity for the visual mesh to be transformed.
    pub visual_entity: Option<Entity>,
    /// Render-root rest rotation composed with its geometry basis from USD.
    /// Independent of the mechanical axle; mesh geometry has no primitive basis.
    /// This is a render-boundary quaternion; physics state remains f64.
    pub visual_base_rotation: Quat,
    /// Resultant normal force from the last physics tick, used for friction calculations.
    pub last_normal_force: f64,
    /// Drives the visible spin of the wheel mesh.
    pub spin_angle: f64,
    /// wheelspin/skid, and free-runs (driven by torque vs bearing drag) in the air.
    pub spin_velocity: f64,
    /// `½·m·r²` that resists changes in spin (unless `moment_of_inertia` is set).
    pub mass: f64,
    /// When `> 0` it overrides the mass-derived `½·m·r²`.
    pub moment_of_inertia: f64,
    /// the hub in its own right — never inferred from the drive torque.
    pub bearing_damping: f64,
    /// Caps the traction torque at `μ·N`, above which the tire breaks loose.
    pub friction_mu: f64,
    /// Longitudinal force per dimensionless slip, N, before Coulomb saturation.
    pub slip_stiffness: f64,
    /// one number instead of two independently-fudged ones.
    pub tire_force: DVec3,
    /// like ice.
    /// Standard PhysX tire lateral stiffness graph evaluated at the contact
    /// load. The force is clamped with the load-dependent Coulomb cone after
    /// the graph law is evaluated.
    pub lateral_stiffness_graph: TireLateralStiffnessGraph,
    /// Lowest speed at which the authored steady-state cornering curve was
    /// validated. This is an evidence boundary, not a fitted transition.
    pub min_validated_speed: f64,
    /// Standard vehicle-level minimum longitudinal slip denominators, m/s.
    pub longitudinal_slip_denominators: LongitudinalSlipDenominators,
    /// traction torque the wheel locks and skids.
    pub brake_torque_max: f64,
    /// Heading rotation axis in the wheel's local frame
    /// (USD `lunco:wheel:headingAxis`, required).
    /// `+Y` (yaw) reproduces a flat-ground car steer; a motorcycle's
    /// raked steering head tilts this (e.g. `(0, cos θ, sin θ)`) so the front
    /// wheel steers about the fork axis, not vertical.
    pub heading_axis: DVec3,
    /// Authored wheel-local cylinder axle (`UsdGeomCylinder.axis`).
    pub axle_axis: DVec3,
    /// Fixed-step steering rotation consumed by wheel physics. The entity's
    /// `Transform` mirrors this value for presentation; physics never reads it
    /// back, since Avian interpolation may change that transform between ticks.
    pub heading_rotation: DQuat,
}

/// **USD is the sole source of a wheel's physical numbers.**
///
/// Physical parameters are zero except for documented standard longitudinal
/// slip-denominator defaults. `Default` supplies the
/// struct-update base for `WheelParams::to_wheel_raycast`, which immediately
/// overwrites all of them from the composed stage via `apply_to_raycast`. The
/// reader (`lunco_usd_sim_authoring::wheel_params`) requires each attribute and reports a
/// collected missing-attribute error, so an unauthored wheel FAILS rather than
/// silently inheriting numbers nobody wrote. A zeroed wheel that ever reaches
/// the world is therefore visibly inert (no drive, no grip) instead of quietly
/// plausible — which is the point.
///
/// `heading_axis` is `+Y` because a zero vector is not a rotation axis at all;
/// `lunco:wheel:headingAxis` is required and overwrites it.
impl Default for WheelRaycast {
    fn default() -> Self {
        Self {
            suspension_port: Entity::PLACEHOLDER,
            drive_port: Entity::PLACEHOLDER,
            speed_port: Entity::PLACEHOLDER,
            heading_port: Entity::PLACEHOLDER,
            wheel_radius: 0.0,
            wheel_width: 0.0,
            visual_entity: None,
            visual_base_rotation: Quat::IDENTITY,
            last_normal_force: 0.0,
            spin_angle: 0.0,
            spin_velocity: 0.0,
            mass: 0.0,
            moment_of_inertia: 0.0,
            bearing_damping: 0.0,
            friction_mu: 0.0,
            slip_stiffness: 0.0,
            lateral_stiffness_graph: TireLateralStiffnessGraph::default(),
            min_validated_speed: 0.0,
            longitudinal_slip_denominators: LongitudinalSlipDenominators::default(),
            tire_force: DVec3::ZERO,
            brake_torque_max: 0.0,
            heading_axis: DVec3::Y,
            axle_axis: DVec3::ZERO,
            heading_rotation: DQuat::IDENTITY,
        }
    }
}

impl WheelRaycast {
    /// The tire's local roll about its axle as a quaternion.
    ///
    /// This is the single source of truth for the wheel's rotation — the visual
    /// mesh is rebuilt from it each tick, and any other system (telemetry,
    /// odometry, networking, a drivetrain model) can read the same orientation
    /// without inspecting the render transform. Built fresh from the wrapped
    /// `spin_angle`, so it never accumulates floating-point drift and is
    /// continuous across the 2π wrap (a 2π quaternion is identity).
    #[inline]
    pub fn spin_quat(&self) -> Quat {
        let axle = self.axle_axis.as_vec3();
        Quat::from_axis_angle(axle, -(self.spin_angle as f32))
    }

    /// The tire's angular velocity about its axle in rad/s (signed: positive is
    /// forward roll). Real physical state — e.g. wheel-encoder odometry can
    /// integrate ground distance as `spin_velocity * wheel_radius`.
    #[inline]
    pub fn axle_angular_velocity(&self) -> f64 {
        self.spin_velocity
    }

    /// Surface (contact-patch) speed implied by the current spin, `ω · r` in m/s.
    /// Compare against chassis ground speed to recover the slip ratio.
    #[inline]
    pub fn surface_speed(&self) -> f64 {
        self.spin_velocity * self.wheel_radius
    }

    /// Complete rotational inertia about the axle in kg·m²: the authored tire
    /// and attached drivetrain assembly inertia (`physxVehicleWheel:moi`), or
    /// the solid-disk estimate `½·m·r²` when the field is omitted. Avian owns
    /// this state at the Modelica co-simulation boundary, so composed USD
    /// wheels author the total rather than integrating a second shaft state.
    /// Returns `None`
    /// when the runtime projection does not contain a finite, positive
    /// physical input or when the combined inertia is not usable.
    #[inline]
    pub fn axle_inertia(&self) -> Option<f64> {
        let tire = if self.moment_of_inertia.is_finite() && self.moment_of_inertia > 0.0 {
            self.moment_of_inertia
        } else {
            if !(self.wheel_radius.is_finite()
                && self.wheel_radius > 0.0
                && self.mass.is_finite()
                && self.mass > 0.0)
            {
                return None;
            }
            0.5 * self.mass * self.wheel_radius * self.wheel_radius
        };
        (tire.is_finite() && tire > 0.0).then_some(tire)
    }
}

/// Refresh the fixed-weld exclusion set once before mobility force systems run.
/// Reuse its allocation and let both suspension and drive consume the same live
/// joint graph for this deterministic tick.
fn refresh_dynamically_fixed_bodies(
    mut held: ResMut<DynamicFixedBodies>,
    fixed_joints: Query<&FixedJoint>,
    bodies: Query<&RigidBody>,
) {
    held.0.clear();
    for joint in fixed_joints.iter() {
        if matches!(bodies.get(joint.body1), Ok(RigidBody::Dynamic))
            && matches!(bodies.get(joint.body2), Ok(RigidBody::Dynamic))
        {
            held.0.insert(joint.body1);
            held.0.insert(joint.body2);
        }
    }
}

fn report_wheel_force_error(
    commands: &mut Commands,
    error: lunco_physics::ForceAdmissionError,
    holds: Option<&mut lunco_physics::PhysicsHolds>,
    faults: Option<&mut lunco_core::RuntimeFaults>,
    physics_time: Option<&mut Time<Physics>>,
) {
    if let Some(holds) = holds {
        holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
    }
    if let Some(time) = physics_time {
        time.pause();
        time.advance_by(std::time::Duration::ZERO);
    }
    if let Some(faults) = faults {
        faults.raise(
            "mobility-native-force-range-invalid",
            Some(error.body),
            "wheel contact force",
            error.detail.clone(),
        );
    }
    lunco_core::trigger_runtime_error(
        commands,
        "mobility-native-force-range-invalid",
        error.detail,
    );
}

/// System solving the vertical suspension dynamics.
///
/// **Logic**: Performs a ray-world intersection check. If a hit is detected
/// within the suspension travel range, it applies an upward force to the
/// parent chassis based on the compression distance and relative velocity.
///
/// **Suspension model**: Spring-damper using Hooke's law:
/// `F = k * compression + c * relative_velocity`
/// Damping is bidirectional — it resists both compression and extension
/// to prevent oscillation. Force is only applied upward (along hit normal)
/// to avoid pulling the chassis into the ground.
///
/// **Geometry**: the wheel entity transform is the AXLE. The ray starts
/// [`strut_offset`] above it — the strut top — and points straight down, so
/// `hit_distance` is the distance from the strut top to the ground and the spring
/// is compressed when `hit_distance < rest_length`. The wheel visual is positioned
/// at `ground_y + wheel_radius`, which in wheel-local Y is the compression, so an
/// unloaded wheel draws exactly at its authored mount.
fn apply_wheel_suspension(
    mut q_wheels: Query<
        (
            Entity,
            Option<&lunco_physics::PhysicsOrderKey>,
            &mut WheelRaycast,
            &Suspension,
            &RayHits,
            &WheelBodyMount,
            Option<&raycast_travel::RaycastStopLoad>,
        ),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    mut bodies: ParamSet<(Query<lunco_physics::AvianForceState>, Query<Forces>)>,
    q_modes: Query<(&RigidBody, Has<RigidBodyDisabled>)>,
    owners: Query<&ColliderOf>,
    mut pending: Local<Vec<lunco_physics::BodyForce>>,
    mut force_scratch: Local<lunco_physics::ForceBatchScratch>,
    mut commands: Commands,
    fixed_dynamic_bodies: Res<DynamicFixedBodies>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: Option<ResMut<Time<Physics>>>,
) {
    let order = match lunco_physics::ordered_physics_entities(
        q_wheels.iter_mut().map(|(entity, key, ..)| (entity, key)),
        "raycast wheel suspension",
    ) {
        Ok(order) => order,
        Err(invalid) => {
            lunco_physics::report_invalid_physics_order(
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
                "physics-wheel-order-invalid",
                &invalid,
            );
            return;
        }
    };
    pending.clear();
    {
        let states = bodies.p0();
        for entity in order {
            let Ok((_, _, mut wheel, susp, hits, mount, stop)) = q_wheels.get_mut(entity) else {
                continue;
            };
            wheel.last_normal_force = 0.0;
            let Ok(carrier) = states.get(mount.body) else {
                continue;
            };
            let (hub, heading) = wheel_hub_pose(
                carrier.position(),
                GridRot(carrier.rotation().as_dquat()),
                mount.local.translation,
                mount.local.rotation * wheel.heading_rotation,
            );
            let contact = hits
                .iter_sorted()
                .find(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12);
            let Some(hit) = contact else {
                continue;
            };
            let support = raycast_travel::support_body(hit.entity, &owners);
            if support == mount.body {
                continue;
            }
            let Ok(support_state) = states.get(support) else {
                continue;
            };
            let distance = f64::from(hit.distance);
            if distance >= susp.rest_length {
                continue;
            }
            let point = raycast_contact_point(
                hub.0,
                heading.0,
                susp.rest_length,
                wheel.wheel_radius,
                distance,
            );
            let relative_velocity = carrier.velocity_at_point(GridPos(point)).as_dvec3()
                - support_state.velocity_at_point(GridPos(point)).as_dvec3();
            let force_magnitude = suspension_force_mag(
                susp.rest_length - distance,
                susp.spring_k,
                -relative_velocity.dot(hit.normal.as_dvec3()),
                susp.damping_c,
            );
            let force = hit.normal.as_dvec3() * force_magnitude;
            wheel.last_normal_force = force_magnitude + stop.map_or(0.0, |load| load.0);
            let carrier_dynamic = q_modes
                .get(mount.body)
                .is_ok_and(|(mode, disabled)| *mode == RigidBody::Dynamic && !disabled)
                && !fixed_dynamic_bodies.0.contains(&mount.body);
            if !carrier_dynamic {
                continue;
            }
            pending.push(lunco_physics::BodyForce {
                local_force: DVec3::ZERO,
                body: mount.body,
                force,
                point: Some(lunco_spatial::coords::GridPos(point)),
                torque: DVec3::ZERO,
                wake: false,
            });
            if q_modes
                .get(support)
                .is_ok_and(|(mode, disabled)| *mode == RigidBody::Dynamic && !disabled)
            {
                pending.push(lunco_physics::BodyForce {
                    local_force: DVec3::ZERO,
                    body: support,
                    force: -force,
                    point: Some(lunco_spatial::coords::GridPos(point)),
                    torque: DVec3::ZERO,
                    wake: false,
                });
            }
        }
        if let Err(error) =
            lunco_physics::prepare_force_batch(&states, &pending, &mut force_scratch)
        {
            report_wheel_force_error(
                &mut commands,
                error,
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
            );
            return;
        }
    }
    if let Err(error) = lunco_physics::apply_force_batch(&mut bodies.p1(), &pending) {
        report_wheel_force_error(
            &mut commands,
            error,
            holds.as_deref_mut(),
            faults.as_deref_mut(),
            physics_time.as_deref_mut(),
        );
    }
}

/// Keep each raycast wheel's avian `Position` in the grid-ABSOLUTE physics frame
/// so its suspension `RayCaster` originates at the true chassis-up strut top —
/// not at the wheel's big_space RENDER-frame `GlobalTransform`. The caster
/// follows the solved hub frame: origin, suspension travel, contact lever and
/// wheel presentation must describe the same strut line on tilted vehicles.
///
/// avian's `update_ray_caster_positions` derives the ray origin from an entity's
/// own `Position`/`Rotation` when present, falling back to its `GlobalTransform`
/// only when they're absent. A raycast wheel now carries them (spawned in
/// `lunco-usd-sim::setup_raycast_wheel`) but is NOT a physics body, so nothing
/// else maintains them: the big_space bridge disables avian's
/// `transform_to_position` and only syncs `BridgeShadow`-carrying bodies (a bare
/// wheel has neither `RigidBody` nor `Collider`). Without this system the ray
/// would cast from the origin-relative render frame and — at an elevated site
/// (≈ +1945 m grid-absolute vs ≈ −53 m render, a ~2 km gap) — miss the terrain
/// collider entirely, leaving `last_normal_force` at 0 so `apply_wheel_drive`
/// has no positive contact load. That is the flat-sandbox-works /
/// elevated-moonbase-fails split: near the origin the two frames coincide.
///
/// We compose the chassis' solved grid-absolute pose with the wheel's local
/// transform via `wheel_hub_pose` — exactly how the suspension/drive force point
/// is built — running AFTER the physics step (fresh chassis pose) and BEFORE the
/// spatial query, so the cast sees this tick's pose.
fn sync_raycast_wheel_physics_pose(
    mut q_wheels: Query<
        (
            Entity,
            &WheelRaycast,
            &mut Position,
            &mut Rotation,
            &mut RayCaster,
            &WheelBodyMount,
            &Suspension,
        ),
        (
            With<WheelRaycast>,
            Without<lunco_core::SimulationObjectPaused>,
        ),
    >,
    q_chassis: Query<
        (
            &Position,
            &Rotation,
            &LinearVelocity,
            &AngularVelocity,
            &ComputedCenterOfMass,
            Option<&ConstantLinearAcceleration>,
        ),
        (With<RigidBody>, Without<WheelRaycast>),
    >,
    physics_time: Res<Time<Physics>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
) {
    for (wheel_entity, wheel, mut wpos, mut wrot, mut raycaster, mount, suspension) in
        q_wheels.iter_mut()
    {
        if let Ok((cpos, crot, velocity, angular_velocity, center, acceleration)) =
            q_chassis.get(mount.body)
        {
            let (hub_pos, hub_rot) = wheel_hub_pose(
                GridPos(cpos.0),
                GridRot(crot.0.as_dquat()),
                mount.local.translation,
                mount.local.rotation * wheel.heading_rotation,
            );
            // Avian transforms this local origin and NEG_Y direction by the
            // same solved hub rotation used by raycast_contact_point below.
            let ray_origin = DVec3::Y * strut_offset(suspension.rest_length, wheel.wheel_radius);
            // Discover a support before the next master step can cross the
            // bump stop. This extends the query only; spring/contact consumers
            // still qualify physical contact against the authored rest length.
            let lever = hub_pos.0 - (cpos.0 + crot.0.as_dquat() * center.0.as_dvec3());
            let point_velocity = velocity.0.as_dvec3() + angular_velocity.0.as_dvec3().cross(lever);
            let step = physics_time.delta_secs_f64();
            let swept_distance = point_velocity.length() * step
                + acceleration.map_or(0.0, |a| 0.5 * f64::from(a.0.length()) * step * step);
            let ray_distance = suspension_ray_max_distance(suspension.rest_length + swept_distance);
            // A NaN rotation or origin poisons the cast and avian asserts on it.
            // avian's `raycast` asserts `origin.is_finite()` and takes the whole
            // app down with it. Do not leave the wheel on an old pose and let the
            // next system interpret that pose as current physics. Record the
            // invalid source as a terminal scene fault; the fault gate pauses
            // force production and the scene lifecycle owns the explicit reset.
            if !hub_pos.0.is_finite()
                || !hub_rot.0.is_finite()
                || !ray_origin.is_finite()
                || !ray_distance.is_finite()
                || ray_distance > f64::from(f32::MAX)
            {
                if let Some(holds) = holds.as_deref_mut() {
                    holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
                }
                if let Some(faults) = faults.as_deref_mut() {
                    if faults.raise(
                        "mobility-nonfinite-wheel-pose",
                        Some(wheel_entity),
                        "raycast wheel",
                        format!(
                            "hub_position={:?}, hub_rotation={:?}, ray_origin={:?}, chassis={:?}",
                            hub_pos.0, hub_rot.0, ray_origin, mount.body,
                        ),
                    ) {
                        error!(
                            "[mobility] terminal runtime failure: non-finite raycast wheel pose on {wheel_entity:?}"
                        );
                    }
                }
                continue;
            }
            // Compare-gate like the bridge's writeback: the hub pose and local
            // origin are deterministic functions of the chassis pose and the
            // wheel's local transform, so an idle chassis recomputes bit-identical
            // values — exact compare, no epsilon. Writing unconditionally dirtied
            // every wheel's physics state on every parked tick.
            if wpos.0 != hub_pos.0 {
                wpos.0 = hub_pos.0;
            }
            if raycaster.origin != ray_origin {
                raycaster.origin = ray_origin;
            }
            // Explicit Avian query scalar boundary; physical parameters remain f64.
            if raycaster.max_distance != ray_distance as f32 {
                raycaster.max_distance = ray_distance as f32;
            }
            if wrot.0 != hub_rot.0.as_quat() {
                wrot.0 = hub_rot.0.as_quat();
            }
        }
    }
}

/// Keep Avian's generic ray-pose update but prevent its serial ray loop from
/// doing the wheel casts. The configured enabled state is restored immediately
/// after the shared query pass.
fn suppress_native_wheel_raycasts(
    mut wheels: Query<(&mut RayCaster, &mut WheelRaycastParallelState), With<WheelRaycast>>,
) {
    for (mut caster, mut state) in &mut wheels {
        state.enabled_before_native_cast = caster.enabled;
        caster.enabled = false;
    }
}

/// Cast wheel rays through Avian's canonical collider trees using Bevy's
/// automatic parallel-query batching. This mirrors Avian's `RayCaster`
/// semantics (filter, solid mode, distance, hit limit, and disabled clearing)
/// while retaining the `RayHits` component consumed by mobility and telemetry.
fn cast_wheel_ray_hits_parallel(
    mut wheels: Query<
        (
            Entity,
            &mut RayCaster,
            &mut RayHits,
            &WheelRaycastParallelState,
        ),
        With<WheelRaycast>,
    >,
    spatial_query: SpatialQuery,
    diagnostics: Option<ResMut<SpatialQueryDiagnostics>>,
) {
    let started = bevy::platform::time::Instant::now();
    wheels
        .par_iter_mut()
        .for_each(|(entity, mut caster, mut hits, state)| {
            if state.enabled_before_native_cast {
                if caster.ignore_self {
                    caster.query_filter.excluded_entities.insert(entity);
                } else {
                    caster.query_filter.excluded_entities.remove(&entity);
                }

                hits.clear();
                let origin = caster.global_origin();
                let direction = caster.global_direction();
                if caster.max_hits == 1 {
                    if let Some(hit) = spatial_query.cast_ray(
                        origin,
                        direction,
                        caster.max_distance,
                        caster.solid,
                        &caster.query_filter,
                    ) {
                        hits.push(hit);
                    }
                } else {
                    spatial_query.ray_hits_callback(
                        origin,
                        direction,
                        caster.max_distance,
                        caster.solid,
                        &caster.query_filter,
                        |hit| {
                            if hits.len() < caster.max_hits as usize {
                                hits.push(hit);
                                true
                            } else {
                                false
                            }
                        },
                    );
                }
            } else if !hits.is_empty() {
                hits.clear();
            }

            caster.enabled = state.enabled_before_native_cast;
        });
    if let Some(mut diagnostics) = diagnostics {
        diagnostics.update_ray_casters = started.elapsed();
    }
}

/// System applying longitudinal drive torque and lateral friction.
///
/// **Theory**: Drive force is applied along the wheel's forward vector at the
/// world-space contact point. Both longitudinal (forward/back) and lateral
/// (side-to-side) friction are computed using a Coulomb friction model where
/// the maximum friction force is `mu * normal_force`. This prevents the rover
/// from sliding like it's on ice and limits drive force to what the tire can
/// actually grip.
fn apply_wheel_drive(
    q_wheels: Query<
        (
            Entity,
            Option<&lunco_physics::PhysicsOrderKey>,
            &WheelRaycast,
            &Suspension,
            &RayHits,
            &WheelBodyMount,
            Option<&WheelHeatPorts>,
        ),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    mut q_ports: ParamSet<(Query<&Port>, Query<&mut Port>)>,
    // Force must land only on a body the solver will integrate. A disabled body
    // (frozen while its program compiles, say) never has its accumulators
    // cleared, so force applied to it is stored, not spent, and discharges in
    // full on the step that eventually runs — see `lunco_physics::Integrable`.
    mut bodies: ParamSet<(Query<lunco_physics::AvianForceState>, Query<Forces>)>,
    q_traits: Query<(
        &RigidBody,
        Has<RigidBodyDisabled>,
        &ComputedMass,
        Option<&InputPorts>,
        Option<&lunco_physics::kinematics::LocalGravity>,
    )>,
    owners: Query<&ColliderOf>,
    mut pending: Local<Vec<lunco_physics::BodyForce>>,
    mut force_scratch: Local<lunco_physics::ForceBatchScratch>,
    mut total_normal_load: Local<std::collections::HashMap<Entity, f64>>,
    mut commands: Commands,
    fixed_dynamic_bodies: Res<DynamicFixedBodies>,
    time: Res<Time<Fixed>>,
    mut holds: Option<ResMut<lunco_physics::PhysicsHolds>>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    mut physics_time: Option<ResMut<Time<Physics>>>,
) {
    let order = match lunco_physics::ordered_physics_entities(
        q_wheels.iter().map(|(entity, key, ..)| (entity, key)),
        "raycast wheel tire force",
    ) {
        Ok(order) => order,
        Err(invalid) => {
            lunco_physics::report_invalid_physics_order(
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
                "physics-wheel-order-invalid",
                &invalid,
            );
            return;
        }
    };
    let dt = time.delta_secs_f64();
    pending.clear();
    {
        let states = bodies.p0();

        // Stable summation in the same physics order used to apply contact forces.
        total_normal_load.clear();
        for entity in &order {
            if let Ok((_, _, wheel, _, hits, mount, _)) = q_wheels.get(*entity) {
                if hits
                    .iter_sorted()
                    .any(|hit| hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12)
                    && contact_has_positive_load(wheel.last_normal_force)
                {
                    *total_normal_load.entry(mount.body).or_default() += wheel.last_normal_force;
                }
            }
        }
        for entity in order {
            let Ok((_, _, wheel, susp, hits, mount, heat_ports)) = q_wheels.get(entity) else {
                continue;
            };
            let parent_entity = mount.body;
            if let (Ok(forces), Ok((body, disabled, mass, inputs, gravity))) =
                (states.get(parent_entity), q_traits.get(parent_entity))
            {
                if disabled {
                    continue;
                }
                if fixed_dynamic_bodies.0.contains(&parent_entity) {
                    continue;
                }
                // Prescribed-motion carriers do not receive dynamic tire forces.
                if *body != RigidBody::Dynamic {
                    continue;
                }
                let drive_torque = q_ports
                    .p0()
                    .get(wheel.drive_port)
                    .map(|port| port.value)
                    .ok();
                if let Some(drive_torque) = drive_torque {
                    let (hub_pos_world, hub_rot_world) = wheel_hub_pose(
                        forces.position(),
                        GridRot(forces.rotation().as_dquat()),
                        mount.local.translation,
                        mount.local.rotation * wheel.heading_rotation,
                    );
                    // Traction only exists when the ray is hitting the ground.
                    let Some(hit) = hits.iter_sorted().find(|hit| {
                        hit.normal.is_finite() && hit.normal.length_squared() > 1.0e-12
                    }) else {
                        continue;
                    };
                    {
                        let normal_force = wheel.last_normal_force;
                        if !contact_has_positive_load(normal_force) {
                            continue;
                        }

                        // The tyre force belongs at the ray's contact point, not at
                        // the hub. Applying it at the hub erased the wheel-radius
                        // moment and was only equivalent on a flat, upright contact.
                        // On a DEM slope that missing moment changes load transfer and
                        // feeds the next suspension sample, producing the observed
                        // forward/back contact limit cycle. The ray starts at the
                        // authored strut top, so reconstruct its hit point in the
                        // same grid-absolute frame as the chassis and ray query.
                        let contact_point = raycast_contact_point(
                            hub_pos_world.0,
                            hub_rot_world.0,
                            susp.rest_length,
                            wheel.wheel_radius,
                            f64::from(hit.distance),
                        );
                        let support = raycast_travel::support_body(hit.entity, &owners);
                        if support == parent_entity {
                            continue;
                        }
                        let Ok(support_state) = states.get(support) else {
                            continue;
                        };
                        let relative_velocity =
                            forces.velocity_at_point(GridPos(contact_point)).as_dvec3()
                                - support_state
                                    .velocity_at_point(GridPos(contact_point))
                                    .as_dvec3();
                        // The tire force was already solved this tick, from the real
                        // contact slip `ω·r − v` and the wheel's own lateral slip —
                        // see `update_wheel_spin`. Applying it is all that is left.
                        // A wheel brake must provide static friction as well as
                        // spin-down torque.  The old path only damped axle spin,
                        // so a rover with zero wheel speed still slid down a
                        // slope: the tire solver produced a few newtons while
                        // gravity supplied the remaining acceleration.  Project
                        // gravity onto this contact plane and distribute the
                        // required support by the measured normal load.  The
                        // authored tire friction coefficient remains the hard
                        // limit; a slope steeper than the tire can hold still
                        // slides naturally.
                        let parking_force = if inputs.is_some_and(|i| i.brake_active) {
                            gravity
                                .map(|g| {
                                    let supported_mass = f64::from(mass.value()) * normal_force
                                        / total_normal_load[&parent_entity];
                                    parking_brake_force_with_velocity(
                                        g.acceleration(),
                                        hit.normal.as_dvec3(),
                                        normal_force,
                                        supported_mass,
                                        supported_mass,
                                        wheel.friction_mu,
                                        relative_velocity,
                                        dt,
                                    )
                                })
                                .unwrap_or(DVec3::ZERO)
                        } else {
                            DVec3::ZERO
                        };
                        // A tire patch is a persistent contact reaction. Reapplying
                        // that reaction must not wake an otherwise settled chassis;
                        // Avian will consume it whenever the body is already in an
                        // active island. A drive or brake command is different: it
                        // is an authored wake-up event and must use the regular
                        // accumulator so a parked rover responds immediately.
                        let commanded =
                            drive_torque != 0.0 || inputs.is_some_and(|input| input.brake_active);
                        // A locked patch has one Coulomb cone. Adding the dynamic
                        // tire force again would count its grip twice.
                        let locked = inputs.is_some_and(|i| i.brake_active)
                            && wheel.spin_velocity == 0.0
                            && gravity.is_some();
                        let force = if locked {
                            let brake_force = (wheel.brake_torque_max - drive_torque.abs())
                                .max(0.0)
                                / wheel.wheel_radius;
                            parking_force.clamp_length_max(brake_force)
                        } else {
                            wheel.tire_force
                        };
                        if locked {
                            if let Some(endpoint) = heat_ports.and_then(|ports| ports.tire) {
                                if let Ok(mut port) = q_ports.p1().get_mut(endpoint) {
                                    // This is the delivered locked-patch force and
                                    // its conjugate slip, not the rolling predictor.
                                    port.value = -force.dot(relative_velocity);
                                }
                            }
                        }
                        pending.push(lunco_physics::BodyForce {
                            local_force: DVec3::ZERO,
                            body: parent_entity,
                            force,
                            point: Some(lunco_spatial::coords::GridPos(contact_point)),
                            torque: DVec3::ZERO,
                            wake: commanded,
                        });
                        if q_traits.get(support).is_ok_and(|(mode, disabled, ..)| {
                            *mode == RigidBody::Dynamic && !disabled
                        }) {
                            pending.push(lunco_physics::BodyForce {
                                local_force: DVec3::ZERO,
                                body: support,
                                force: -force,
                                point: Some(lunco_spatial::coords::GridPos(contact_point)),
                                torque: DVec3::ZERO,
                                wake: commanded,
                            });
                        }
                    }
                }
            }
        }
        if let Err(error) =
            lunco_physics::prepare_force_batch(&states, &pending, &mut force_scratch)
        {
            report_wheel_force_error(
                &mut commands,
                error,
                holds.as_deref_mut(),
                faults.as_deref_mut(),
                physics_time.as_deref_mut(),
            );
            return;
        }
    }
    if let Err(error) = lunco_physics::apply_force_batch(&mut bodies.p1(), &pending) {
        report_wheel_force_error(
            &mut commands,
            error,
            holds.as_deref_mut(),
            faults.as_deref_mut(),
            physics_time.as_deref_mut(),
        );
    }
}

/// Realize a controller-produced wheel heading for a raycast wheel.
///
/// Physical wheels receive the same authored scalar through a standard
/// `PhysicsRevoluteJoint` `angle` port. A raycast wheel has no joint, so this
/// small realization rotates its physics probe and visual basis. It performs
/// no command interpretation, rate limiting, or vehicle-type dispatch; those
/// belong to the authored controller.
fn apply_wheel_heading(
    mut q_wheels: Query<
        (&mut WheelRaycast, &mut Transform, &WheelBodyMount),
        Without<lunco_core::SimulationObjectPaused>,
    >,
    q_ports: Query<&Port>,
    q_chassis: Query<&RigidBody, With<RigidBody>>,
) {
    for (mut wheel, mut transform, mount) in q_wheels.iter_mut() {
        // Predict-own: this chain runs on a client too. Skip wheels of a
        // `Kinematic` chassis (replicated rovers this peer does NOT own), whose
        // local steer ports are stale and would point the wheels wrong.
        if let Ok(body) = q_chassis.get(mount.body) {
            if matches!(body, RigidBody::Kinematic) {
                continue;
            }
        }
        let Ok(heading) = q_ports.get(wheel.heading_port) else {
            continue;
        };
        if !heading.value.is_finite() {
            continue;
        }
        // Rotate about the authored wheel-local heading axis. The input is the
        // final angle in radians, not a normalized vehicle command. The
        // authored wheel attitude is already part of `WheelBodyMount.local`;
        // this quaternion carries only the changing steering rotation.
        let axis = wheel.heading_axis.normalize_or_zero();
        if axis == DVec3::ZERO || !axis.is_finite() {
            continue;
        }
        let rotation = DQuat::from_axis_angle(axis, heading.value);
        wheel.heading_rotation = rotation;
        let presentation_rotation = rotation.as_quat();
        if transform.rotation != presentation_rotation {
            transform.rotation = presentation_rotation;
        }
    }
}

/// Drive interpretation for an authored `PhysicsDriveAPI` relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Reflect)]
pub enum DifferentialDriveType {
    /// Coefficients describe a generalized force/torque.
    Force,
    /// Coefficients describe a generalized acceleration.
    Acceleration,
}

/// Authored `PhysicsDriveAPI` parameters for a gear relation. `lunco-usd-sim`
/// reads these fields into a `PendingDifferential` and
/// `resolve_differential_coupling` attaches this component.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct DifferentialCoupling {
    /// Explicit authored hinge relationships, in ratio order.
    pub hinges: [Entity; 2],
    /// The chassis body the two rockers pivot against (reaction torque target).
    pub chassis: Entity,
    /// Left rocker body.
    pub rocker_a: Entity,
    /// Right rocker body.
    pub rocker_b: Entity,
    /// Authored `physxGearJoint:gearRatio` — the `r` in `θ_a = r·θ_b`.
    pub ratio: f64,
    /// Target for `θ_a − r·θ_b` (rad).
    pub rest_offset: f64,
    /// Target relation velocity (rad/s).
    pub target_velocity: f64,
    /// Coupling stiffness (N·m per rad of relation error in force mode).
    pub stiffness: f64,
    /// Coupling damping (N·m·s per rad/s of relation velocity in force mode).
    pub damping: f64,
    /// Maximum generalized force/torque. Infinity is the standard USD fallback.
    pub max_force: f64,
    /// Whether the authored coefficients are acceleration-based.
    pub drive_type: DifferentialDriveType,
}

/// Read the angular state of a native Avian revolute joint using the same
/// joint-frame construction as its XPBD solver. `Rotation` is the frame-start
/// body orientation and `delta_rotation` is the accumulated substep correction.
fn native_revolute_state(
    joint: &RevoluteJoint,
    body1_rotation: Quat,
    body2_rotation: Quat,
    body1_delta: Quat,
    body2_delta: Quat,
) -> Option<(f64, DVec3)> {
    lunco_physics::joint::revolute_coordinate(
        joint,
        (body1_delta * body1_rotation).as_dquat(),
        (body2_delta * body2_rotation).as_dquat(),
    )
}

#[derive(Resource, Default)]
struct DifferentialSolverOrder(Vec<Entity>);

fn prepare_differential_order(
    couplings: Query<(Entity, Option<&lunco_physics::PhysicsOrderKey>), With<DifferentialCoupling>>,
    mut order: ResMut<DifferentialSolverOrder>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut time: ResMut<Time<Physics>>,
) {
    order.0.clear();
    match lunco_physics::ordered_physics_entities(couplings.iter(), "gear drive") {
        Ok(entities) => order.0 = entities,
        Err(invalid) => lunco_physics::report_invalid_physics_order(
            Some(&mut holds),
            Some(&mut faults),
            Some(&mut time),
            "physics-gear-order-invalid",
            &invalid,
        ),
    }
}

/// Integrate each authored gear drive once per physical substep, before hinge
/// alignment. Joints and gears share the same implicit coordinate law and the
/// later position-to-velocity projection.
fn solve_differential_gear(
    order: Res<DifferentialSolverOrder>,
    q_coupling: Query<&DifferentialCoupling>,
    q_revolute: Query<&RevoluteJoint, Without<JointDisabled>>,
    q_solver: Query<
        (
            &RigidBody,
            &Rotation,
            Has<Sleeping>,
            Option<&SolverBodyIndex>,
        ),
        Without<RigidBodyDisabled>,
    >,
    mut solver_bodies: ResMut<SolverBodies>,
    time: Res<Time<Substeps>>,
    mut holds: ResMut<lunco_physics::PhysicsHolds>,
    mut faults: ResMut<lunco_core::RuntimeFaults>,
    mut physics_time: ResMut<Time<Physics>>,
) {
    if faults.active() {
        return;
    }
    for &entity in &order.0 {
        let result = (|| -> Result<(), &str> {
            let coupling = q_coupling
                .get(entity)
                .map_err(|_| "prepared gear drive no longer exists")?;
            let [joint_a, joint_b] = q_revolute
                .get_many(coupling.hinges)
                .map_err(|_| "gear requires its two explicitly authored active revolute joints")?;
            if joint_a.body1 != coupling.chassis
                || joint_a.body2 != coupling.rocker_a
                || joint_b.body1 != coupling.chassis
                || joint_b.body2 != coupling.rocker_b
            {
                return Err("gear hinge endpoints differ from admitted body topology");
            }
            let endpoints = [coupling.rocker_a, coupling.rocker_b, coupling.chassis];
            let states = q_solver
                .get_many(endpoints)
                .map_err(|_| "gear requires three distinct admitted rigid bodies")?;
            let indices =
                states.map(|(_, _, _, index)| index.copied().filter(SolverBodyIndex::is_valid));
            for (i, ((body, _, sleeping, _), index)) in states.iter().zip(indices).enumerate() {
                if let Some(index) = index {
                    if solver_bodies.get_entity(index) != Some(endpoints[i]) {
                        return Err("gear solver index does not identify its admitted endpoint");
                    }
                    if indices[..i].contains(&Some(index)) {
                        return Err("gear endpoints share a solver index");
                    }
                } else if !body.is_static() && !sleeping {
                    return Err("active movable gear endpoint has no solver state");
                }
            }
            let [(_, ra, _, _), (_, rb, _, _), (_, rc, _, _)] = states;
            let access = solver_bodies.access();
            // The entity checks above establish in-bounds, distinct indices.
            // The serial gear pass holds the resource exclusively; native static
            // and sleeping endpoints use Avian's documented zero-mobility state.
            let [a, b, c] = indices.map(|index| {
                index.map(|index| unsafe {
                    (
                        access.body_unchecked_mut(index),
                        &*access.inertia_unchecked_mut(index),
                    )
                })
            });
            let (mut dummy_a, mut dummy_b, mut dummy_c) =
                (SolverBody::DUMMY, SolverBody::DUMMY, SolverBody::DUMMY);
            let (sa, ia) = a.unwrap_or((&mut dummy_a, &SolverBodyInertia::DUMMY));
            let (sb, ib) = b.unwrap_or((&mut dummy_b, &SolverBodyInertia::DUMMY));
            let (sc, ic) = c.unwrap_or((&mut dummy_c, &SolverBodyInertia::DUMMY));
            let (angle_a, axis_a) =
                native_revolute_state(joint_a, rc.0, ra.0, sc.delta_rotation, sa.delta_rotation)
                    .ok_or("gear first hinge frame is invalid")?;
            let (angle_b, axis_b) =
                native_revolute_state(joint_b, rc.0, rb.0, sc.delta_rotation, sb.delta_rotation)
                    .ok_or("gear second hinge frame is invalid")?;
            let r = coupling.ratio;
            if !r.is_finite() || r == 0.0 {
                return Err("gear ratio must be finite and nonzero");
            }
            if ![coupling.stiffness, coupling.damping]
                .into_iter()
                .all(|value| value.is_finite() && value >= 0.0 && value <= f64::from(f32::MAX))
            {
                return Err("gear gains exceed the native solver range");
            }
            let model = match coupling.drive_type {
                DifferentialDriveType::Force => MotorModel::ForceBased {
                    stiffness: coupling.stiffness as f32,
                    damping: coupling.damping as f32,
                },
                DifferentialDriveType::Acceleration => MotorModel::AccelerationBased {
                    stiffness: coupling.stiffness as f32,
                    damping: coupling.damping as f32,
                },
            };
            integrate_coordinate_drive(
                [sa, sb, sc],
                [ia, ib, ic],
                [Vec3::ZERO; 3],
                [axis_a, -r * axis_b, -axis_a + r * axis_b].map(DVec3::as_vec3),
                CoordinateDrive {
                    model,
                    target_position: coupling.rest_offset,
                    target_velocity: coupling.target_velocity,
                    max_force: coupling.max_force,
                },
                angle_a - r * angle_b,
                time.delta_secs_f64(),
            )?;
            Ok(())
        })();
        if let Err(detail) = result {
            holds.set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
            physics_time.pause();
            physics_time.advance_by(std::time::Duration::ZERO);
            if faults.raise(
                "physics-gear-drive-invalid",
                Some(entity),
                "gear drive",
                detail,
            ) {
                error!("[physics] refusing invalid gear drive: {detail}");
            }
            return;
        }
    }
}

#[cfg(test)]
mod wheel_raycast_parallel_tests {
    use super::*;
    use bevy::time::TimeUpdateStrategy;
    use std::time::Duration;

    #[test]
    fn raycast_wheel_physics_pose_ignores_interpolated_transform() {
        let mut app = App::new();
        app.init_resource::<Time<Physics>>();
        app.add_systems(FixedPostUpdate, sync_raycast_wheel_physics_pose);

        let chassis_position = DVec3::new(12.0, 4.0, -8.0);
        let chassis_rotation = Quat::from_rotation_z(0.35);
        let chassis = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(chassis_position),
                Rotation(chassis_rotation),
            ))
            .id();

        let authored_mount = lunco_core::DTransform {
            translation: DVec3::new(1.0, -0.25, 0.75),
            rotation: DQuat::from_rotation_x(0.2),
            scale: DVec3::ONE,
        };
        let fixed_heading = DQuat::from_rotation_y(0.4);
        let render_sample = Quat::from_rotation_z(-0.9);
        let wheel = app
            .world_mut()
            .spawn((
                WheelRaycast {
                    wheel_radius: 0.35,
                    heading_rotation: fixed_heading,
                    ..default()
                },
                Position(DVec3::ZERO),
                Rotation(Quat::IDENTITY),
                RayCaster::new(DVec3::ZERO, Dir3::NEG_Y),
                WheelBodyMount {
                    body: chassis,
                    local: authored_mount,
                },
                Suspension {
                    rest_length: 0.8,
                    travel_distance: 0.8,
                    spring_k: 0.0,
                    damping_c: 0.0,
                    local_axis: DVec3::Y,
                },
                Transform::from_rotation(render_sample),
            ))
            .id();

        let (expected_position, expected_rotation) = wheel_hub_pose(
            GridPos(chassis_position),
            GridRot(chassis_rotation.as_dquat()),
            authored_mount.translation,
            authored_mount.rotation * fixed_heading,
        );
        let expected_ray_origin = DVec3::Y * strut_offset(0.8, 0.35);

        app.world_mut().run_schedule(FixedPostUpdate);

        assert_eq!(
            app.world().get::<Position>(wheel).unwrap().0,
            expected_position.0
        );
        assert_eq!(
            app.world().get::<RayCaster>(wheel).unwrap().origin,
            expected_ray_origin
        );
        assert_eq!(
            app.world().get::<Rotation>(wheel).unwrap().0,
            expected_rotation.0.as_quat()
        );
        assert_eq!(
            app.world().get::<Transform>(wheel).unwrap().rotation,
            render_sample
        );

        let (_, render_rotation) = wheel_hub_pose(
            GridPos(chassis_position),
            GridRot(chassis_rotation.as_dquat()),
            authored_mount.translation,
            authored_mount.rotation * render_sample.as_dquat(),
        );
        let render_ray_origin = render_rotation.0 * DVec3::Y * strut_offset(0.8, 0.35);
        assert_ne!(render_ray_origin, expected_rotation.0 * expected_ray_origin);
        // The native cast and the suspension force must use the same contact
        // point despite vehicle tilt, mount rotation and steering heading.
        let distance = 0.63;
        let cast_contact = expected_position.0 + expected_rotation.0 * expected_ray_origin
            - expected_rotation.0 * DVec3::Y * distance;
        assert!(
            (cast_contact
                - raycast_contact_point(
                    expected_position.0,
                    expected_rotation.0,
                    0.8,
                    0.35,
                    distance
                ))
            .length()
                < 1.0e-12
        );
    }

    #[test]
    fn parallel_wheel_cast_matches_native_query_and_preserves_disabled_state() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            PhysicsPlugins::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::mesh::MeshPlugin,
        ))
        .add_systems(
            FixedPostUpdate,
            suppress_native_wheel_raycasts
                .before(SpatialQuerySystems)
                .run_if(any_with_component::<WheelRaycast>),
        )
        .add_systems(
            FixedPostUpdate,
            cast_wheel_ray_hits_parallel
                .after(SpatialQuerySystems)
                .run_if(any_with_component::<WheelRaycast>),
        );
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            34,
        )));
        app.finish();

        app.world_mut().spawn((
            RigidBody::Static,
            Position(DVec3::new(0.0, -0.5, 0.0)),
            Rotation(Quat::IDENTITY),
            Transform::from_xyz(0.0, -0.5, 0.0),
            Collider::cuboid(10.0, 1.0, 10.0),
        ));
        let wheel = app
            .world_mut()
            .spawn((
                WheelRaycast::default(),
                Position(DVec3::new(0.0, 2.0, 0.0)),
                Rotation(Quat::IDENTITY),
                Transform::from_xyz(0.0, 2.0, 0.0),
                RayCaster::new(DVec3::ZERO, Dir3::NEG_Y)
                    .with_max_distance(5.0)
                    .with_max_hits(1),
            ))
            .id();
        let generic = app
            .world_mut()
            .spawn((
                Position(DVec3::new(2.0, 2.0, 0.0)),
                Rotation(Quat::IDENTITY),
                Transform::from_xyz(2.0, 2.0, 0.0),
                RayCaster::new(DVec3::ZERO, Dir3::NEG_Y)
                    .with_max_distance(5.0)
                    .with_max_hits(1),
            ))
            .id();

        for _ in 0..4 {
            app.update();
        }

        let wheel_hits = app.world().get::<RayHits>(wheel).unwrap();
        assert_eq!(wheel_hits.len(), 1);
        assert!(
            (wheel_hits[0].distance - 2.0).abs() < 1.0e-9,
            "wheel hit distance was {}; position={:?}; origin={:?}; direction={:?}",
            wheel_hits[0].distance,
            app.world().get::<Position>(wheel).unwrap().0,
            app.world().get::<RayCaster>(wheel).unwrap().global_origin(),
            app.world()
                .get::<RayCaster>(wheel)
                .unwrap()
                .global_direction()
        );
        assert_eq!(app.world().get::<RayHits>(generic).unwrap().len(), 1);
        assert_eq!(wheel_hits, app.world().get::<RayHits>(generic).unwrap());
        assert!(app.world().get::<RayCaster>(wheel).unwrap().enabled);

        app.world_mut()
            .get_mut::<RayCaster>(wheel)
            .unwrap()
            .disable();
        for _ in 0..2 {
            app.update();
        }
        assert!(app.world().get::<RayHits>(wheel).unwrap().is_empty());
        assert!(!app.world().get::<RayCaster>(wheel).unwrap().enabled);
        assert_eq!(app.world().get::<RayHits>(generic).unwrap().len(), 1);
    }
}

/// Authored raycast-wheel suspension parameters.
///
/// Jointed suspension does not use this component: a physical prismatic joint
/// reads its standard `PhysicsDriveAPI:linear` directly through the USD/Avian
/// bridge, leaving Avian as the sole solver for that degree of freedom.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component, Default)]
pub struct Suspension {
    /// target static length of the strut.
    pub rest_length: f64,
    /// Maximum compression from the uncompressed wheel mount, m.
    pub travel_distance: f64,
    /// Stiffness (N/m).
    pub spring_k: f64,
    /// Dampening (Ns/m).
    pub damping_c: f64,
    /// Direction of extension.
    pub local_axis: DVec3,
}

impl Default for Suspension {
    fn default() -> Self {
        Self {
            rest_length: 0.4,
            travel_distance: 0.4,
            spring_k: 50000.0,
            damping_c: 2000.0,
            local_axis: DVec3::Y,
        }
    }
}

// ── Drive command ports ─────────────────────────────────────────────────────────

/// Derive each endpoint's input surface from its intent binding: for any entity
/// that has both [`InputPorts`] and a [`lunco_control_core::ControlBinding`], ensure every
/// bound port exists in `InputPorts.values` (seeded `0.0`).
///
/// This is what lets the command vocabulary be **data, not a Rust literal**: a
/// vessel's `Controls` profile (→ its `ControlBinding`) declares exactly which
/// command ports it accepts, and the strict command backend then admits writes to
/// those and no others. Additive (never removes keys) and idempotent, so it's safe to
/// run on `Changed<ControlBinding>` regardless of which reader stamped the binding or
/// the surface, and regardless of spawn order.
fn sync_input_ports(
    mut q: Query<
        (&lunco_control_core::ControlBinding, &mut InputPorts),
        Or<(
            Changed<lunco_control_core::ControlBinding>,
            Added<InputPorts>,
        )>,
    >,
) {
    for (binding, mut inputs) in q.iter_mut() {
        for port in binding.ports() {
            if !inputs.values.contains_key(port) {
                inputs.values.insert(port.to_string(), 0.0);
            }
        }
    }
}

// ── Vehicle brake boundary ────────────────────────────────────────────────────

/// Apply the vessel-wide brake command independently of drive allocation.
///
/// A vehicle may expose an [`OutputPorts`] index, but braking is a
/// mechanism shared by all authored drive networks: it owns
/// [`InputPorts::brake_active`] (used by the tire solve) and the discrete
/// `brake` actuator port. The drive allocation and steering laws remain in the
/// authored Modelica/Rhai network; this function only realizes the brake input
/// at the generic vehicle boundary.
fn apply_vehicle_brake(
    mut q: Query<(&mut InputPorts, Option<&OutputPorts>)>,
    mut q_ports: Query<&mut Port>,
) {
    for (mut inputs, actuators) in &mut q {
        let brake_active = inputs.cmd("brake") > 0.5;
        inputs.brake_active = brake_active;
        if let Some(actuators) = actuators {
            if let Some(port_b) = actuators.get("brake") {
                if let Ok(mut port) = q_ports.get_mut(port_b) {
                    port.value = if inputs.brake_active { 1.0 } else { 0.0 };
                }
            }
        }
    }
}

#[cfg(test)]
mod raycast_wheel_mass_tests {
    //! The vehicle must mass the same whichever `drivetrain` variant realizes its
    //! wheels. See [`fold_raycast_wheel_mass`].
    use super::*;

    #[test]
    fn automatic_carrier_uses_prepared_mass_and_publishes_complete_override() {
        let mut app = App::new();
        app.add_systems(Update, fold_raycast_wheel_mass);
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                ComputedMass::new(1000.0),
                ComputedAngularInertia::new(Vec3::splat(10.0)),
                ComputedCenterOfMass(Vec3::ZERO),
            ))
            .id();
        assert!(app.world().get::<Mass>(body).is_none());
        app.world_mut().spawn((
            WheelRaycast {
                mass: 25.0,
                ..default()
            },
            WheelBodyMount {
                body,
                local: lunco_core::DTransform {
                    translation: DVec3::Y,
                    ..lunco_core::DTransform::IDENTITY
                },
            },
            lunco_physics::PhysicsOrderKey("inline/automatic-carrier/wheel".to_owned()),
        ));
        app.update();
        assert_eq!(app.world().get::<Mass>(body).unwrap().0, 1025.0);
        assert!(app.world().get::<NoAutoMass>(body).is_some());
        assert!(app.world().get::<NoAutoAngularInertia>(body).is_some());
        let center = app.world().get::<CenterOfMass>(body).unwrap().0;
        assert!((center.y - 25.0 / 1025.0).abs() < 1e-7);
        let tensor = app.world().get::<AngularInertia>(body).unwrap().tensor();
        assert!(
            (tensor.diagonal()
                - Vec3::new(10.0 + 25_000.0 / 1025.0, 10.0, 10.0 + 25_000.0 / 1025.0,))
            .abs()
            .max_element()
                < 1e-4
        );
    }

    /// Build a skid-rover-shaped chassis with four raycast wheels at the mounts
    /// `skid_rover.usda` authors — (±1.0, −0.65, ±1.225), the SAME mounts the
    /// `physical` variant's wheel bodies get, because the wheel prim is the axle
    /// in both realizations. Runs the fold; returns (mass, inertia, centre of mass).
    fn fold_a_four_wheel_rover(wheel_mass: f64) -> (f32, Vec3, Vec3) {
        let mut app = App::new();
        app.add_systems(Update, fold_raycast_wheel_mass);

        let chassis = app
            .world_mut()
            .spawn((
                MobilityRoot,
                OutputPorts::default(),
                Mass(1000.0),
                NoAutoMass,
                AngularInertia {
                    principal: Vec3::new(1028.0, 1354.0, 341.0),
                    ..default()
                },
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
            ))
            .id();

        for (x, z) in [(-1.0, -1.225), (1.0, -1.225), (-1.0, 1.225), (1.0, 1.225)] {
            let local = lunco_core::DTransform {
                translation: DVec3::new(x, -0.65, z),
                ..lunco_core::DTransform::IDENTITY
            };
            let wheel = app
                .world_mut()
                .spawn((
                    WheelRaycast {
                        mass: wheel_mass,
                        wheel_radius: 0.4,
                        ..default()
                    },
                    lunco_physics::PhysicsOrderKey(format!("test-wheel/{x}/{z}")),
                    local.as_bevy_transform(),
                    WheelBodyMount {
                        body: chassis,
                        local,
                    },
                    ChildOf(chassis),
                ))
                .id();
            let _ = wheel;
        }

        app.update();

        let mass = app.world().get::<Mass>(chassis).unwrap().0;
        let inertia = app
            .world()
            .get::<AngularInertia>(chassis)
            .unwrap()
            .tensor()
            .diagonal();
        let com = app
            .world()
            .get::<CenterOfMass>(chassis)
            .map(|c| c.0)
            .unwrap_or(Vec3::ZERO);
        (mass, inertia, com)
    }

    #[test]
    fn a_raycast_rover_masses_the_same_as_its_physical_twin() {
        // The physical twin is chassis 1000 kg + four 25 kg wheel bodies. The
        // raycast rover's wheels are kinematic proxies avian never weighs, so
        // without the fold the same USD file massed 1000 kg — a 10% vehicle
        // change caused by nothing but a variant switch.
        let (mass, _, _) = fold_a_four_wheel_rover(25.0);
        assert!((mass - 1100.0).abs() < 1e-3, "expected 1100 kg, got {mass}");
    }

    #[test]
    fn the_mass_acts_where_the_wheels_hang_it() {
        // A physical rover's four wheel bodies hang at the axle and PULL THE
        // COMBINED CENTRE OF MASS DOWN — avian does that arithmetic for free
        // because they are bodies. The raycast rover's proxies are not, so its
        // mass kept acting at the chassis centre: same total, same tensor, wrong
        // place. CoM height is load transfer, so the two rovers would still have
        // cornered differently with every other number matched.
        //
        //   (1000·0 + 4·25·(−0.65)) / 1100 = −65/1100 = −0.0590909… m
        let (_, _, com) = fold_a_four_wheel_rover(25.0);
        assert!(
            (com.y as f64 + 65.0 / 1100.0).abs() < 1e-6,
            "expected CoM y = -0.0590909, got {}",
            com.y
        );
        // x and z must cancel: the mounts are symmetric (±1.0, ±1.225), and a
        // rover whose mass drifted sideways would pull in a straight line.
        assert!(
            com.x.abs() < 1e-6 && com.z.abs() < 1e-6,
            "symmetric mounts must cancel, got {com:?}"
        );
    }

    #[test]
    fn the_wheels_arrive_at_their_mounts_not_at_the_centre_of_mass() {
        // Mass alone was measured and made the suite WORSE (heading 56.3° → 61.7°
        // against a physical twin at 51°): a heavier rover that was no harder to
        // turn. Each wheel must bring its parallel-axis term `m·d²` at its
        // authored mount, which grows the yaw tensor FASTER than the mass.
        let (mass, inertia, _) = fold_a_four_wheel_rover(25.0);

        // Measured from the COMBINED centre (y = −0.0590909), not from the body
        // origin — the authored tensor is about the chassis centre and that centre
        // has just moved, so the chassis contributes a term of its own.
        //   chassis: 1000·(0.0590909²) = 3.4917 about x and z, 0 about y
        //   wheel at (±1.0, −0.5909091, ±1.225): m·(y²+z²), m·(x²+z²), m·(x²+y²)
        //     = 25·(0.349174 + 1.500625), 25·(1.0 + 1.500625), 25·(1.0 + 0.349174)
        //     = 46.2450, 62.5156, 33.7293   → ×4 → 184.980, 250.063, 134.917
        let expected = Vec3::new(
            1028.0 + 3.4917355 + 184.97986,
            1354.0 + 250.0625,
            341.0 + 3.4917355 + 134.91736,
        );
        assert!(
            (inertia - expected).abs().max_element() < 1e-2,
            "expected {expected:?}, got {inertia:?}"
        );

        // The point of the whole exercise: yaw inertia must rise FASTER than mass,
        // or the rover gets heavier without getting harder to turn.
        let mass_ratio = mass / 1000.0;
        let yaw_ratio = inertia.y / 1354.0;
        assert!(
            yaw_ratio > mass_ratio,
            "yaw inertia grew {yaw_ratio:.4}× but mass grew {mass_ratio:.4}×"
        );
    }

    #[test]
    fn a_wheel_whose_parameters_have_not_landed_yet_defers_the_fold() {
        // `WheelParams::apply_to_raycast` can land a tick after the component. A
        // wheel still reading zero mass means the vehicle is not ready; wait
        // for its parameters rather than publishing a partial aggregate.
        let (mass, inertia, com) = fold_a_four_wheel_rover(0.0);
        assert_eq!(mass, 1000.0, "folded before the wheel parameters arrived");
        assert_eq!(inertia, Vec3::new(1028.0, 1354.0, 341.0));
        assert_eq!(
            com,
            Vec3::ZERO,
            "centre of mass moved before the fold was due"
        );
    }

    #[test]
    fn folding_twice_does_not_double_the_rover() {
        let mut app = App::new();
        app.add_systems(Update, fold_raycast_wheel_mass);
        let chassis = app
            .world_mut()
            .spawn((
                MobilityRoot,
                OutputPorts::default(),
                Mass(1000.0),
                NoAutoMass,
                AngularInertia::new(Vec3::ONE),
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
            ))
            .id();
        app.world_mut().spawn((
            WheelRaycast {
                mass: 25.0,
                ..default()
            },
            lunco_physics::PhysicsOrderKey("test-wheel/one".to_owned()),
            WheelBodyMount {
                body: chassis,
                local: lunco_core::DTransform::IDENTITY,
            },
            Transform::default(),
            ChildOf(chassis),
        ));

        app.update();
        app.update();
        app.update();

        assert!((app.world().get::<Mass>(chassis).unwrap().0 - 1025.0).abs() < 1e-3);
    }

    #[test]
    fn articulated_wheel_mass_is_found_through_its_carrier() {
        let mut app = App::new();
        app.add_systems(Update, fold_raycast_wheel_mass);
        let chassis = app
            .world_mut()
            .spawn((
                MobilityRoot,
                OutputPorts::default(),
                Mass(1000.0),
                NoAutoMass,
                AngularInertia::new(Vec3::ONE),
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
            ))
            .id();
        let rocker = app
            .world_mut()
            .spawn((
                Mass(50.0),
                NoAutoMass,
                AngularInertia::new(Vec3::ONE),
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
                Transform::from_translation(Vec3::new(0.0, -0.2, 0.5)),
                ChildOf(chassis),
            ))
            .id();
        let local = lunco_core::DTransform {
            translation: DVec3::new(0.0, -0.65, -1.2),
            ..lunco_core::DTransform::IDENTITY
        };
        app.world_mut().spawn((
            WheelRaycast {
                mass: 25.0,
                ..default()
            },
            lunco_physics::PhysicsOrderKey("test-wheel/articulated".to_owned()),
            WheelBodyMount {
                body: rocker,
                local,
            },
            local.as_bevy_transform(),
            ChildOf(rocker),
        ));

        app.update();

        let chassis_mass = app.world().get::<Mass>(chassis).unwrap().0;
        let rocker_mass = app.world().get::<Mass>(rocker).unwrap().0;
        let com = app
            .world()
            .get::<CenterOfMass>(rocker)
            .map(|c| c.0)
            .unwrap_or(Vec3::ZERO);
        assert!((chassis_mass - 1000.0).abs() < 1e-3);
        assert!((rocker_mass - 75.0).abs() < 1e-3);
        assert!((com.y + 0.65 * 25.0 / 75.0).abs() < 1e-6);
        assert!((com.z + 1.2 * 25.0 / 75.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod force_law_tests {
    //! Regression guards for the numerically-sensitive wheel force laws. Each
    //! test pins a property whose violation previously caused a jitter or a
    //! broken control (the comments name the bug).
    use super::*;
    use bevy::math::{DQuat, DVec3};
    use lunco_spatial::coords::VehicleFrame;

    #[test]
    fn ray_contact_point_follows_authored_strut_geometry() {
        let hub = DVec3::new(2.0, 3.0, 4.0);
        let rotation = DQuat::from_rotation_z(core::f64::consts::FRAC_PI_2);

        // The authored ray starts 0.8 m along local +Y and travels 0.25 m
        // along local -Y. Under this rotation those directions are -X and +X.
        let contact = raycast_contact_point(hub, rotation, 1.2, 0.4, 0.25);

        assert!((contact - DVec3::new(1.45, 3.0, 4.0)).length() < 1.0e-12);
    }

    #[test]
    fn suspension_query_includes_the_authored_rest_endpoint() {
        let rest = 0.7;
        let query = suspension_ray_max_distance(rest);

        assert!(query > rest);
        assert!((query - rest - 1.0e-6).abs() < 1.0e-12);
        assert_eq!(suspension_ray_max_distance(0.0), 0.0);
        assert!(suspension_ray_max_distance(f64::NAN).is_nan());
    }

    #[test]
    fn authored_brake_command_reaches_the_generic_brake_port() {
        let mut app = App::new();
        app.add_systems(Update, apply_vehicle_brake);

        let brake_port = app.world_mut().spawn(Port::default()).id();
        let mut inputs = InputPorts::new(&["throttle", "steer", "brake"]);
        inputs.values.insert("brake".to_string(), 1.0);
        let vehicle = app
            .world_mut()
            .spawn((
                inputs,
                OutputPorts::new(std::collections::HashMap::from([(
                    "brake".to_string(),
                    brake_port,
                )])),
            ))
            .id();

        app.update();

        assert!(app.world().get::<InputPorts>(vehicle).unwrap().brake_active);
        assert_eq!(app.world().get::<Port>(brake_port).unwrap().value, 1.0);
    }

    #[test]
    fn generated_network_control_surface_releases_brake_without_output_ports() {
        let mut app = App::new();
        app.add_systems(Update, apply_vehicle_brake);

        let mut inputs = InputPorts::new(&["throttle", "steer", "brake"]);
        inputs.values.insert("brake".to_string(), 0.0);
        inputs.brake_active = true;
        let vehicle = app.world_mut().spawn(inputs).id();

        app.update();

        assert!(!app.world().get::<InputPorts>(vehicle).unwrap().brake_active);
    }

    // ── Single-track lean: contact-plane traction basis ─────────────────────
    #[test]
    fn contact_basis_upright_matches_flat_wheel() {
        // Upright wheel: contact normal = world up. Basis must equal the raw
        // wheel forward/right (so existing rovers are unchanged).
        let (f, r) = contact_plane_basis(
            VehicleFrame::FORWARD_LOCAL,
            VehicleFrame::RIGHT_LOCAL,
            DVec3::Y,
        );
        assert!(
            (f - VehicleFrame::FORWARD_LOCAL).length() < 1e-9,
            "forward changed: {f:?}"
        );
        assert!(
            (r - VehicleFrame::RIGHT_LOCAL).length() < 1e-9,
            "right changed: {r:?}"
        );
    }

    #[test]
    fn contact_basis_leaned_lies_in_contact_plane() {
        // Cambered contact: normal tilted 22° off vertical. Both basis vectors
        // must lie in the plane ⟂ to the normal, stay unit, and be orthogonal.
        let n = DVec3::new(0.0, 1.0, 0.4).normalize();
        let (f, r) = contact_plane_basis(VehicleFrame::FORWARD_LOCAL, VehicleFrame::RIGHT_LOCAL, n);
        assert!(
            f.dot(n).abs() < 1e-9,
            "forward not in contact plane: {}",
            f.dot(n)
        );
        assert!(
            r.dot(n).abs() < 1e-9,
            "right not in contact plane: {}",
            r.dot(n)
        );
        assert!((f.length() - 1.0).abs() < 1e-9 && (r.length() - 1.0).abs() < 1e-9);
        assert!(f.dot(r).abs() < 1e-9, "forward/right not orthogonal");
    }

    #[test]
    fn jointed_contact_basis_retains_motor_direction_through_vertical_pitch() {
        for sign in [-1.0, 1.0] {
            for degrees in [0.0_f64, 70.0, 89.0, 90.0, 91.0, 95.0, 110.0] {
                let rotation = bevy::math::DQuat::from_rotation_x(degrees.to_radians());
                let heading = rotation * VehicleFrame::FORWARD_LOCAL;
                let axle = rotation * DVec3::X;
                let (forward, right) = jointed_contact_plane_basis(heading, axle, sign, DVec3::Y);
                assert!((forward - sign * DVec3::Z).length() < 1e-12);
                assert!(forward.dot(DVec3::Y).abs() < 1e-12);
                assert!((right.length() - 1.0).abs() < 1e-12);
                // Positive coordinate speed must roll in positive traction
                // direction, with zero material slip at the bottom contact.
                let radius = 0.465;
                let velocity = forward * radius;
                let omega = axle * sign;
                assert!((velocity + omega.cross(-radius * DVec3::Y)).length() < 1e-12);
            }
        }
    }

    // ── suspension_force_mag: bounded, never negative, damps both ways ───────
    #[test]
    fn suspension_force_is_nonnegative_and_bounded() {
        let (k, c) = (8000.0, 2800.0);
        let x = 0.05;
        let spring = x * k;
        assert!(
            suspension_force_mag(x, k, -1000.0, c) >= 0.0,
            "ground can't pull"
        );
        assert!(
            suspension_force_mag(x, k, 1000.0, c) <= 2.0 * spring + 1e-9,
            "bounded"
        );
        assert!(
            (suspension_force_mag(x, k, 0.0, c) - spring).abs() < 1e-9,
            "at rest = spring"
        );
    }

    #[test]
    fn suspension_damps_the_rebound_half_cycle() {
        // REGRESSION: `(spring + c·v).max(0)` dropped ALL damping on fast rebound
        // → an undamped suspension limit-cycle (the forward+turn jitter). A
        // moderate rebound must still be damped (force below spring), not clamped.
        let (k, c) = (8000.0, 2800.0);
        let x = 0.1;
        let spring = x * k; // 800
        let f = suspension_force_mag(x, k, -0.1, c); // c·v = -280, within ±spring
        assert!(f < spring, "rebound is damped");
        assert!(f > 0.0, "not clamped to zero");
        assert!((f - (spring - 280.0)).abs() < 1e-9);
    }
}

/// fine RK4 step (≈ the Modelica answer to many digits for this non-stiff system)
/// and compare against the real `suspension_force_mag`, stepped with the
/// production scheme (semi-implicit Euler at dt = 1/60). See
/// `docs/architecture/28-modelica-realtime-physics.md` §8 (Step 2).
///
/// What it establishes:
/// 1. **Physics + integration agree** in the gentle regime (the clamp inactive) —
///    the Rust law tracks the continuous reference to sub-cm.
/// 2. **The fixed law settles** — no sustained limit-cycle (the dead-band / `.max(0)`
///    bugs would ring forever).
/// 3. **The bound is the fix** — on a hard landing the production law caps the
///    force at `2·k·χ`, while the old `.max(0)` cliff lets it spike (the 27 kN-class
///    transient the jitter work removed). The oracle is sensitive to that exact
///    regression.
#[cfg(test)]
mod oracle {
    use super::*;

    // Quarter-car: m·χ̈ = m·g − F(χ, χ̇). χ = compression (m), χ̇ = compression rate.
    const M: f64 = 250.0; // sprung mass per wheel — quarter of a 1000 kg chassis
    const G: f64 = 9.81;
    const DT_SIM: f64 = 1.0 / 60.0; // the real FixedUpdate step
    const DT_FINE: f64 = 1.0e-4; // RK4 reference step (≈ the adaptive-solver answer)

    /// The continuous physics the Rust law approximates (QuarterCar.mo): ideal
    /// linear spring-damper, no clamp. Zero force when out of contact (χ ≤ 0).
    fn reference_force(chi: f64, chi_dot: f64, k: f64, c: f64) -> f64 {
        if chi > 0.0 {
            k * chi + c * chi_dot
        } else {
            0.0
        }
    }

    /// Integrate the reference with RK4 at a fine step → ground truth trajectory of
    /// compression χ. State = (χ, v=χ̇); χ′ = v, v′ = g − F/m.
    fn reference_chi(k: f64, c: f64, chi0: f64, v0: f64, secs: f64) -> Vec<f64> {
        let n = (secs / DT_FINE) as usize;
        let d = |chi: f64, v: f64| (v, G - reference_force(chi, v, k, c) / M);
        let (mut chi, mut v) = (chi0, v0);
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let (a1, b1) = d(chi, v);
            let (a2, b2) = d(chi + 0.5 * DT_FINE * a1, v + 0.5 * DT_FINE * b1);
            let (a3, b3) = d(chi + 0.5 * DT_FINE * a2, v + 0.5 * DT_FINE * b2);
            let (a4, b4) = d(chi + DT_FINE * a3, v + DT_FINE * b3);
            chi += DT_FINE / 6.0 * (a1 + 2.0 * a2 + 2.0 * a3 + a4);
            v += DT_FINE / 6.0 * (b1 + 2.0 * b2 + 2.0 * b3 + b4);
            out.push(chi);
        }
        out
    }

    /// Integrate a force law with the PRODUCTION scheme — semi-implicit Euler at
    /// dt = 1/60, exactly as `apply_wheel_suspension` runs. Returns the per-step
    /// compression and the applied force, so callers can probe both trajectory and
    /// force transients.
    fn step_law<F: Fn(f64, f64) -> f64>(
        force: F,
        chi0: f64,
        v0: f64,
        secs: f64,
    ) -> (Vec<f64>, Vec<f64>) {
        let n = (secs / DT_SIM) as usize;
        let (mut chi, mut v) = (chi0, v0);
        let (mut chis, mut forces) = (Vec::with_capacity(n), Vec::with_capacity(n));
        for _ in 0..n {
            let f = if chi > 0.0 { force(chi, v) } else { 0.0 };
            v += DT_SIM * (G - f / M); // semi-implicit: velocity first…
            chi += DT_SIM * v; //          …then position with the new velocity
            chis.push(chi);
            forces.push(f);
        }
        (chis, forces)
    }

    // The production force law used by every raycast wheel.
    fn fixed(k: f64, c: f64) -> impl Fn(f64, f64) -> f64 {
        move |chi, v| suspension_force_mag(chi, k, v, c)
    }

    fn max_abs_dev(a: &[f64], b_fine: &[f64]) -> f64 {
        // a is sampled at DT_SIM, b_fine at DT_FINE; compare at matching times.
        let ratio = (DT_SIM / DT_FINE).round() as usize;
        a.iter()
            .enumerate()
            .map(|(i, &x)| {
                let j = ((i + 1) * ratio - 1).min(b_fine.len() - 1);
                (x - b_fine[j]).abs()
            })
            .fold(0.0_f64, f64::max)
    }

    #[test]
    fn fixed_law_tracks_the_continuous_reference_in_the_gentle_regime() {
        // Production params, a soft settle from below equilibrium (χ_eq = 0.3066 m).
        // The clamp never engages, so the Rust law IS the continuous physics and the
        // only gap is fixed-step integration error — must stay sub-cm.
        let (k, c) = (8000.0, 2800.0);
        let (rust, _f) = step_law(fixed(k, c), 0.20, 0.0, 3.0);
        let reference = reference_chi(k, c, 0.20, 0.0, 3.0);
        let dev = max_abs_dev(&rust, &reference);
        let chi_eq = M * G / k;
        println!(
            "[oracle] gentle: max|χ_rust−χ_ref| = {dev:.5} m, χ_end = {:.4} (eq {chi_eq:.4})",
            rust.last().unwrap()
        );
        assert!(
            dev < 8.0e-3,
            "fixed law diverges from continuous reference: {dev} m"
        );
        assert!(
            (rust.last().unwrap() - chi_eq).abs() < 2.0e-3,
            "must settle at m·g/k"
        );
    }

    #[test]
    fn fixed_law_settles_no_limit_cycle() {
        // An under-damped configuration still has to decay under the authored
        // force law; this is the physical settling property the runtime needs.
        let (k, c) = (8000.0, 400.0);
        let (rust, _f) = step_law(fixed(k, c), 0.15, 0.0, 5.0);
        let win = rust.len() / 5;
        let p2p = |s: &[f64]| {
            s.iter().cloned().fold(f64::MIN, f64::max) - s.iter().cloned().fold(f64::MAX, f64::min)
        };
        let early = p2p(&rust[..win]);
        let late = p2p(&rust[rust.len() - win..]);
        println!("[oracle] settle: early p2p {early:.4} m, late p2p {late:.5} m");
        assert!(
            late < 0.15 * early,
            "ringing must decay (limit-cycle guard): {late} vs {early}"
        );
    }

    #[test]
    fn bounded_law_caps_a_hard_landing_force() {
        // Hard landing: χ starts at 0 with a fast downward (compressing) velocity.
        // The authored law must remain finite and respect its displacement-scaled
        // force bound on every active contact step.
        let (k, c) = (8000.0, 2800.0);
        let v_impact = 12.0;
        let (chi_fixed, f_fixed) = step_law(fixed(k, c), 0.0, v_impact, 0.5);
        for (chi, force) in chi_fixed.iter().zip(f_fixed.iter()).skip(1) {
            assert!(chi.is_finite() && force.is_finite());
            assert!(*force >= 0.0);
            assert!(*force <= 2.0 * k * chi.max(0.0) + 1.0);
        }
    }
}

/// Marker component added to an entity representing the suspension piston visual.
/// Stores the initial Y coordinate so that we can offset it relative to the wheel's
/// visual displacement.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SuspensionPiston {
    pub initial_y: f32,
}

/// Marker component added to an entity representing the suspension spring visual.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct SuspensionSpring;

/// Animate USD-authored visual suspension components (casing, piston, spring)
/// based on the raycast wheel's dynamic suspension compression.
///
/// The `SuspensionPiston` / `SuspensionSpring` marker components are stamped at
/// LOAD time by `process_usd_sim_prim_read` (lunco-usd-sim) from the prim's
/// authored `lunco:suspensionVisual:role` token — NOT detected here by name and
/// NOT lazy-attached. This system only reads the markers and
/// translates/scales the visuals along the Y-axis relative to their rest
/// positions. See `assets/components/mobility/suspensions/standard.usda`.
fn update_suspension_visuals(
    q_wheels: Query<(&WheelRaycast, &Suspension, &RayHits, Option<&Children>)>,
    mut q_piston: Query<(&mut Transform, &SuspensionPiston), Without<WheelRaycast>>,
    mut q_spring: Query<
        &mut Transform,
        (
            With<SuspensionSpring>,
            Without<WheelRaycast>,
            Without<SuspensionPiston>,
        ),
    >,
) {
    for (wheel, susp, hits, children) in q_wheels.iter() {
        let Some(children) = children else {
            continue;
        };

        let mut current_distance = susp.rest_length;
        if let Some(hit) = hits.iter_sorted().next() {
            if f64::from(hit.distance) < susp.rest_length {
                current_distance = f64::from(hit.distance);
            }
        }

        // Hub and strut top in WHEEL-LOCAL Y (the wheel prim is the axle): the hub
        // rises by the compression, the top is fixed at `strut_offset`.
        let hub_y = susp.rest_length - current_distance;
        let top_y = strut_offset(susp.rest_length, wheel.wheel_radius);
        let delta_y = susp.rest_length - current_distance;

        for child in children.iter() {
            if Some(child) == wheel.visual_entity {
                continue;
            }

            if let Ok((mut tf, piston)) = q_piston.get_mut(child) {
                let translation_y = (piston.initial_y as f64 + delta_y) as f32;
                if tf.translation.y != translation_y {
                    tf.translation.y = translation_y;
                }
            } else if let Ok(mut tf) = q_spring.get_mut(child) {
                let rest_susp_length = strut_offset(susp.rest_length, wheel.wheel_radius);
                if rest_susp_length > 1e-4 {
                    let current_susp_length = (current_distance - wheel.wheel_radius).max(0.0);
                    let scale_y = (current_susp_length / rest_susp_length) as f32;
                    if tf.scale.y != scale_y {
                        tf.scale.y = scale_y;
                    }
                    // The coil spans hub → strut top, so it sits at their midpoint.
                    let translation_y = ((hub_y + top_y) / 2.0) as f32;
                    if tf.translation.y != translation_y {
                        tf.translation.y = translation_y;
                    }
                }
            }
        }
    }
}

impl lunco_core::SceneResource for DynamicFixedBodies {}

impl lunco_core::SceneResource for DifferentialSolverOrder {}
