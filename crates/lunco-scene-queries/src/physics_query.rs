//! `QueryPhysicsState` — read the generic live physics admission state.
//!
//! This is intentionally a small diagnostic surface rather than a vehicle
//! adapter. It reports the body mode, velocities, sleeping/readiness markers,
//! authored USD identity, and any published support footprint. Twin-authored
//! tests can therefore explain a release or admission failure through the
//! public query path without reaching into private ECS components.

use avian3d::prelude::{
    AngularVelocity, ComputedAngularInertia, ComputedCenterOfMass, ComputedMass, LinearVelocity,
    RigidBody, Sleeping,
};
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::{GlobalEntityId, PhysicsStatePending, PhysicsStateReady};
use lunco_physics::{
    PhysicsSupportFootprint, PhysicsSupportState, PhysicsWheelContact,
    PhysicsWheelRaycastFilter,
};
use lunco_usd_avian_contracts::ShouldBeDynamic;
use lunco_usd_bevy_scene::UsdPrimPath;

/// `QueryPhysicsState { id }` → generic live body/admission state.
pub struct QueryPhysicsStateProvider;

/// `PhysicsPerformance` — read-only solver timing and topology population.
///
/// The query is intentionally generic and available to Rhai/HTTP/headless
/// hosts.  It reports the last Avian step together with the live ECS population
/// so a Twin can distinguish a slow solver from a stalled co-simulation worker
/// or a leaking joint lifecycle without reaching into private resources. The
/// counters are sampled on demand; callers should not poll this on every fixed
/// step unless they are deliberately profiling the query itself.
pub struct PhysicsPerformanceProvider;

impl ApiQueryProvider for PhysicsPerformanceProvider {
    fn name(&self) -> &'static str {
        "PhysicsPerformance"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let Some(timing) = world.get_resource::<avian3d::diagnostics::PhysicsTotalDiagnostics>()
        else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "PhysicsPerformance: Avian total diagnostics are not installed".to_string(),
            );
        };

        let mut bodies = 0usize;
        let mut dynamic = 0usize;
        let mut sleeping = 0usize;
        let mut colliders = 0usize;
        let mut joints = 0usize;
        let mut sensors = 0usize;
        let mut joint_links = 0usize;
        let mut pending_joints = 0usize;
        let mut detach_requests = 0usize;
        let mut detach_sets = 0usize;
        let mut detached_joint_paths = 0usize;
        let mut entities = 0usize;
        for entity in world.iter_entities() {
            entities += 1;
            if entity.contains::<RigidBody>() {
                bodies += 1;
                if entity.get::<RigidBody>() == Some(&RigidBody::Dynamic) {
                    dynamic += 1;
                }
                if entity.contains::<Sleeping>() {
                    sleeping += 1;
                }
            }
            if entity.contains::<avian3d::prelude::Collider>() {
                colliders += 1;
                if entity.contains::<avian3d::prelude::Sensor>() {
                    sensors += 1;
                }
            }
            if entity.contains::<lunco_physics::PhysicsJointLink>() {
                joint_links += 1;
            }
            if entity.contains::<lunco_physics::PhysicsJointPending>() {
                pending_joints += 1;
            }
            if entity.contains::<lunco_physics::PhysicsJointDetachRequested>() {
                detach_requests += 1;
            }
            if let Some(detached) = entity.get::<lunco_physics::PhysicsJointDetachSet>() {
                detach_sets += 1;
                detached_joint_paths += detached.joint_paths.len();
            }
            // Avian exposes each constraint as its own component rather than
            // a common `Joint` marker. Count every shipped constraint type so
            // the result remains useful without coupling the API to one joint
            // implementation.
            if entity.contains::<avian3d::prelude::FixedJoint>()
                || entity.contains::<avian3d::prelude::PrismaticJoint>()
                || entity.contains::<avian3d::prelude::RevoluteJoint>()
                || entity.contains::<avian3d::prelude::SphericalJoint>()
                || entity.contains::<avian3d::prelude::DistanceJoint>()
            {
                joints += 1;
            }
        }

        // The native component count and graph count must agree after an
        // attach/detach transaction. A mismatch is the useful signal here:
        // it catches an edge left in Avian's island graph even when the joint
        // entity itself was already despawned.
        let Some(joint_graph) =
            world.get_resource::<avian3d::dynamics::solver::joint_graph::JointGraph>()
        else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "PhysicsPerformance: Avian joint graph is not installed".to_string(),
            );
        };
        let joint_graph_edges = joint_graph.graph().edge_count();

        ApiResponse::ok(serde_json::json!({
            "step_number": timing.step_number,
            "step_time_ms": timing.step_time.as_secs_f64() * 1000.0,
            "entities": entities,
            "bodies": bodies,
            "dynamic_bodies": dynamic,
            "sleeping_bodies": sleeping,
            "colliders": colliders,
            "sensors": sensors,
            "joints": joints,
            "joint_links": joint_links,
            "joint_graph_edges": joint_graph_edges,
            "pending_joints": pending_joints,
            "detach_requests": detach_requests,
            "detach_sets": detach_sets,
            "detached_joint_paths": detached_joint_paths,
        }))
    }
}

impl ApiQueryProvider for QueryPhysicsStateProvider {
    fn name(&self) -> &'static str {
        "QueryPhysicsState"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw) = params.get("id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "QueryPhysicsState: `id` (entity id) required".to_string(),
            );
        };
        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(raw)))
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("Entity {raw} not found"),
            );
        };
        // The API registry has already resolved this id against the live World.
        // Keep the guard so a stale registry entry is still reported as a
        // normal query miss, then read the components directly. Constructing a
        // fresh `QueryState` for every script/API sample rebuilds archetype
        // access metadata on the hot path; these are immutable component reads
        // and do not need a system query at all.
        if world.get_entity(entity).is_err() {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("Entity {raw} not found"),
            );
        }
        let body = world.get::<RigidBody>(entity);
        let linear = world.get::<LinearVelocity>(entity);
        let angular = world.get::<AngularVelocity>(entity);
        let sleeping = world.get::<Sleeping>(entity).is_some();
        let ready = world.get::<PhysicsStateReady>(entity).is_some();
        let pending = world.get::<PhysicsStatePending>(entity).is_some();
        let admission_requested = world.get::<ShouldBeDynamic>(entity).is_some();
        let disabled = world
            .get::<avian3d::prelude::RigidBodyDisabled>(entity)
            .is_some();
        let collider = world.get::<avian3d::prelude::Collider>(entity).is_some();
        let mass = world.get::<ComputedMass>(entity);
        let center_of_mass = world.get::<ComputedCenterOfMass>(entity);
        let inertia = world.get::<ComputedAngularInertia>(entity);
        let support = world.get::<PhysicsSupportFootprint>(entity);
        let support_state = world.get::<PhysicsSupportState>(entity);
        let prim_path = world.get::<UsdPrimPath>(entity);
        let body_mode = body.map(|body| match body {
            RigidBody::Dynamic => "dynamic",
            RigidBody::Kinematic => "kinematic",
            RigidBody::Static => "static",
        });
        let support_contacts = support
            .map(|footprint| {
                footprint
                    .0
                    .iter()
                    .map(|contact| {
                        serde_json::json!({
                            "local_offset": [
                                contact.local_offset.x,
                                contact.local_offset.y,
                                contact.local_offset.z
                            ],
                            "radius_m": contact.radius,
                            "probe_origin": [
                                contact.probe_origin.x,
                                contact.probe_origin.y,
                                contact.probe_origin.z
                            ],
                            "probe_direction": [
                                contact.probe_direction.x,
                                contact.probe_direction.y,
                                contact.probe_direction.z
                            ],
                            "probe_length_m": contact.probe_length,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // These are effective native raycast samples, not authored support
        // probes. Keep the lookup on the physics-owned snapshot component so
        // this generic query remains independent of a particular mobility
        // implementation. The result is sorted by USD wheel path to make
        // Rhai/headless evidence deterministic across ECS archetype order.
        let registry = world.get_resource::<ApiEntityRegistry>();
        let mut wheel_contacts = world
            .iter_entities()
            .filter_map(|wheel_entity| {
                let contact = wheel_entity.get::<PhysicsWheelContact>()?;
                if contact.owner != entity {
                    return None;
                }
                let wheel_id = registry
                    .and_then(|registry| registry.api_id_for(wheel_entity.id()))
                    .map(|id| id.get());
                let wheel_path = wheel_entity
                    .get::<UsdPrimPath>()
                    .map(|path| path.path.as_str().to_owned());
                let hit_id = contact.hit_entity.and_then(|hit| {
                    registry
                        .and_then(|registry| registry.api_id_for(hit))
                        .map(|id| id.get())
                });
                let hit_path = contact.hit_entity.and_then(|hit| {
                    world
                        .get::<UsdPrimPath>(hit)
                        .map(|path| path.path.as_str().to_owned())
                });
                let filter = wheel_entity.get::<PhysicsWheelRaycastFilter>();
                let filter_exclusions = filter
                    .map(|filter| {
                        filter
                            .excluded_entities
                            .iter()
                            .map(|excluded| {
                                let api_id = registry
                                    .and_then(|registry| registry.api_id_for(*excluded))
                                    .map(|id| id.get());
                                let usd_prim_path = world
                                    .get::<UsdPrimPath>(*excluded)
                                    .map(|path| path.path.as_str().to_owned());
                                serde_json::json!({
                                    "api_id_available": api_id.is_some(),
                                    "api_id": api_id.unwrap_or(0),
                                    "usd_prim_path_available": usd_prim_path.is_some(),
                                    "usd_prim_path": usd_prim_path.unwrap_or_default(),
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Some(serde_json::json!({
                    "wheel_api_id": wheel_id,
                    "wheel_usd_prim_path": wheel_path,
                    "contact_valid": contact.contact_valid,
                    // Rhai telemetry uses `true` for unit pulses. Do not put
                    // nullable fields directly in mission evidence: explicit
                    // availability flags keep an absent hit distinguishable
                    // from a real boolean and make the verdict portable to
                    // JSON/Python consumers as well.
                    "hit_available": contact.hit_entity.is_some(),
                    "hit_api_id_available": hit_id.is_some(),
                    "hit_api_id": hit_id.unwrap_or(0),
                    "hit_usd_prim_path_available": hit_path.is_some(),
                    "hit_usd_prim_path": hit_path.unwrap_or_default(),
                    "distance_available": contact.distance_m.is_some(),
                    "distance_m": contact.distance_m.unwrap_or(0.0),
                    "normal": [contact.normal.x, contact.normal.y, contact.normal.z],
                    "normal_force_n": contact.normal_force_n,
                    "suspension_rest_length_m": contact.suspension_rest_length_m,
                    "suspension_compression_m": contact.suspension_compression_m,
                    "tire_force_n": [
                        contact.tire_force.x,
                        contact.tire_force.y,
                        contact.tire_force.z
                    ],
                    "ray_hit_count": contact.ray_hit_count,
                    "valid_ray_hit_count": contact.valid_ray_hit_count,
                    "raycast_filter_excluded_entity_count": contact.raycast_filter_excluded_entity_count,
                    "raycast_filter_exclusions_available": filter.is_some(),
                    "raycast_filter_exclusions": filter_exclusions,
                    "ray_origin": [contact.ray_origin.x, contact.ray_origin.y, contact.ray_origin.z],
                    "ray_direction": [
                        contact.ray_direction.x,
                        contact.ray_direction.y,
                        contact.ray_direction.z
                    ],
                    "ray_max_distance_m": contact.ray_max_distance_m,
                    "sample_tick": contact.sample_tick,
                }))
            })
            .collect::<Vec<_>>();
        wheel_contacts.sort_by(|left, right| {
            left.get("wheel_usd_prim_path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .cmp(
                    right
                        .get("wheel_usd_prim_path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                )
        });
        ApiResponse::ok(serde_json::json!({
            "api_id": raw,
            "usd_prim_path": prim_path.map(|path| path.path.as_str()),
            "body_mode": body_mode,
            "linear_velocity_mps": linear.map(|velocity| [velocity.0.x, velocity.0.y, velocity.0.z]),
            "angular_velocity_radps": angular.map(|velocity| [velocity.0.x, velocity.0.y, velocity.0.z]),
            "sleeping": sleeping,
            "physics_state_ready": ready,
            "physics_state_pending": pending,
            "physics_admission_requested": admission_requested,
            "rigid_body_disabled": disabled,
            "collider_present": collider,
            "mass_kg": mass.map(|mass| mass.value()),
            "center_of_mass_m": center_of_mass
                .map(|center| [center.0.x, center.0.y, center.0.z]),
            "inertia_principal_kgm2": inertia.map(|inertia| {
                let (principal, _) = inertia.principal_angular_inertia_with_local_frame();
                [principal.x, principal.y, principal.z]
            }),
            // A footprint is authored support geometry; a contact count is a
            // live result from the owning physics realization. Keep them
            // separate so callers cannot mistake a declared probe for ground
            // support. `null` means this body has no runtime support-state
            // producer, not that it is grounded zero times.
            "support_footprint_count": support_contacts.len(),
            "support_contact_count": support_state.map(|state| state.active_contact_count),
            "support_sample_tick": support_state.map(|state| state.sample_tick),
            "support_contacts": support_contacts,
            "wheel_contacts": wheel_contacts,
        }))
    }
}

/// Register the provider beside `QueryEntity` and `QueryUsdPrim`.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let world = app.world_mut();
    world.register_component::<RigidBody>();
    world.register_component::<LinearVelocity>();
    world.register_component::<AngularVelocity>();
    world.register_component::<Sleeping>();
    world.register_component::<PhysicsStateReady>();
    world.register_component::<PhysicsStatePending>();
    world.register_component::<ShouldBeDynamic>();
    world.register_component::<avian3d::prelude::RigidBodyDisabled>();
    world.register_component::<avian3d::prelude::Collider>();
    world.register_component::<ComputedMass>();
    world.register_component::<ComputedCenterOfMass>();
    world.register_component::<ComputedAngularInertia>();
    world.register_component::<PhysicsSupportFootprint>();
    world.register_component::<PhysicsSupportState>();
    world.register_component::<PhysicsWheelContact>();
    world.register_component::<PhysicsWheelRaycastFilter>();
    world.register_component::<UsdPrimPath>();
    world
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryPhysicsStateProvider);
    world
        .resource_mut::<ApiQueryRegistry>()
        .register(PhysicsPerformanceProvider);
}
