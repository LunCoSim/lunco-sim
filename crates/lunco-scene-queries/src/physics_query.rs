//! `QueryPhysicsState` — read the generic live physics admission state.
//!
//! This is intentionally a small diagnostic surface rather than a vehicle
//! adapter. It reports the body mode, velocities, sleeping/readiness markers,
//! authored USD identity, and any published support footprint. Twin-authored
//! tests can therefore explain a release or admission failure through the
//! public query path without reaching into private ECS components.

use avian3d::prelude::{AngularVelocity, LinearVelocity, RigidBody, Sleeping};
use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::{GlobalEntityId, PhysicsStatePending, PhysicsStateReady};
use lunco_physics::PhysicsSupportFootprint;
use lunco_usd_bevy_scene::UsdPrimPath;

/// `QueryPhysicsState { id }` → generic live body/admission state.
pub struct QueryPhysicsStateProvider;

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
        let Some(mut state) = QueryState::<(
            Option<&RigidBody>,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
            Has<Sleeping>,
            Has<PhysicsStateReady>,
            Has<PhysicsStatePending>,
            Option<&PhysicsSupportFootprint>,
            Option<&UsdPrimPath>,
        )>::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryPhysicsState: world state unavailable".to_string(),
            );
        };
        let Ok((body, linear, angular, sleeping, ready, pending, support, prim_path)) =
            state.get(world, entity)
        else {
            return ApiResponse::error(
                ApiErrorCode::EntityNotFound,
                format!("Entity {raw} not found"),
            );
        };
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
        ApiResponse::ok(serde_json::json!({
            "api_id": raw,
            "usd_prim_path": prim_path.map(|path| path.path.as_str()),
            "body_mode": body_mode,
            "linear_velocity_mps": linear.map(|velocity| [velocity.0.x, velocity.0.y, velocity.0.z]),
            "angular_velocity_radps": angular.map(|velocity| [velocity.0.x, velocity.0.y, velocity.0.z]),
            "sleeping": sleeping,
            "physics_state_ready": ready,
            "physics_state_pending": pending,
            "support_contact_count": support_contacts.len(),
            "support_contacts": support_contacts,
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
    world.register_component::<PhysicsSupportFootprint>();
    world.register_component::<UsdPrimPath>();
    world
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryPhysicsStateProvider);
}
