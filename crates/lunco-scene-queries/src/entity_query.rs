//! `QueryEntity` — read one scene entity's identity and pose.
//!
//! The frame contract belongs to the crate that owns the scene verbs, so the read
//! side sits beside the write side: `QueryEntity` reports exactly the active
//! physics frame `TransformEntity` accepts. Query a pose, hand it straight back,
//! and the object does not move. The concrete BigSpace grid remains an internal
//! implementation detail owned by `ActivePhysicsFrame`.
//!
//! The command uses the canonical API envelope:
//! `{"type":"ExecuteCommand","command":"QueryEntity","params":{"id":…}}`.

use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry, SimulationQueryReadScope};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::{ApiQueryError, ApiQueryResult, api_param_u64};
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_celestial::CelestialBody;
use lunco_core::{CatalogEntryId, GlobalEntityId, UsdPrimKind};
use lunco_scene_catalog::catalog::SpawnCatalog;
use lunco_usd_bevy_scene::UsdPrimPath;

/// `QueryEntity { id }` → that entity's name, kind, pose.
pub struct QueryEntityProvider;

impl ApiQueryProvider for QueryEntityProvider {
    fn name(&self) -> &'static str {
        "QueryEntity"
    }

    fn simulation_read_scope(&self, _params: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::EntityTargets
    }

    fn simulation_entity_reads(&self, params: &ApiValue) -> Vec<GlobalEntityId> {
        api_param_u64(params, "id")
            .map(GlobalEntityId::from_raw)
            .into_iter()
            .collect()
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(raw) = api_param_u64(params, "id") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "QueryEntity: `id` (entity id) required",
            ));
        };
        let Some(entity) = world
            .get_resource::<ApiEntityRegistry>()
            .and_then(|r| r.resolve(&GlobalEntityId::from_raw(raw)))
        else {
            return Err(ApiQueryError::new(
                ApiErrorCode::EntityNotFound,
                format!("Entity {raw} not found"),
            ));
        };

        let Some(mut q_meta) = QueryState::<(
            Option<&Name>,
            Option<&lunco_core::markers::Callsign>,
            Has<lunco_control_core::ControlBinding>,
            Option<&CelestialBody>,
            Option<&Transform>,
            Option<&CatalogEntryId>,
            Option<&UsdPrimKind>,
            Option<&UsdPrimPath>,
        )>::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "QueryEntity: world state unavailable",
            ));
        };
        let Some(mut poses) = lunco_physics::SimulationPoseReadState::try_new(world) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                "QueryEntity: active physics frame unavailable",
            ));
        };

        let (name, callsign, accepts_commands, body, transform, catalog_id, usd_kind, prim_path) =
            q_meta
                .get(world, entity)
                .unwrap_or((None, None, false, None, None, None, None, None));
        let kind = usd_kind.map(|kind| kind.0.as_str()).unwrap_or("untyped");
        let origin = catalog_id
            .and_then(|id| {
                world
                    .get_resource::<SpawnCatalog>()
                    .and_then(|catalog| catalog.get(id.0.as_str()))
            })
            .map(|entry| entry.origin.as_api_value());

        let Some((pos, rot)) = poses.pose(world, entity) else {
            return Err(ApiQueryError::new(
                ApiErrorCode::InternalError,
                format!("QueryEntity: entity {raw} is not connected to the active physics frame"),
            ));
        };
        let pos = pos.0;
        let rot = rot.0.as_quat();
        // Object scale is authored on the object itself. Ancestor/grid scale is
        // not part of the rigid active-frame pose contract.
        let scale = transform.map_or(Vec3::ONE, |tf| tf.scale);
        // Euler YXZ (yaw, pitch, roll) — matches the sun / steering authoring
        // convention, handier than a quat.
        let (yaw, pitch, roll) = rot.to_euler(EulerRot::YXZ);
        Ok(Some(api_value!({
            "api_id": raw,
            "name": lunco_core::entity_display_name(name, callsign, catalog_id),
            "type": kind,
            "control_bound": accepts_commands,
            "celestial_body": body.is_some(),
            "catalog_id": catalog_id.map(|id| id.0.as_str()),
            "origin": origin,
            "usd_prim_path": prim_path.map(|path| path.path.as_str()),
            "position": api_value!([pos.x, pos.y, pos.z]),
            // The frame `position` is in, named on the wire: a client holding a
            // bare triple has no way to know whether it may hand it back.
            "position_frame": "active_physics",
            "rotation": api_value!([rot.x, rot.y, rot.z, rot.w]),
            "euler": api_value!([yaw, pitch, roll]),
            "scale": api_value!([scale.x, scale.y, scale.z]),
        })))
    }
}

/// Register the provider. Called by `SpawnCommandPlugin`, so any binary with the
/// scene verbs also answers `QueryEntity` — including the headless server.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let world = app.world_mut();
    // `QueryState::try_new` needs every component in the query to be present
    // in the world's component registry, including optional metadata. The
    // provider owns this vocabulary; relying on a particular USD scene or
    // another plugin to have spawned one of these components makes an absent
    // optional field turn the entire query into an internal error.
    world.register_component::<Name>();
    world.register_component::<lunco_core::markers::Callsign>();
    world.register_component::<lunco_control_core::ControlBinding>();
    world.register_component::<CelestialBody>();
    world.register_component::<Transform>();
    world.register_component::<CatalogEntryId>();
    world.register_component::<UsdPrimKind>();
    world.register_component::<UsdPrimPath>();
    world
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryEntityProvider);
}
