//! `QueryEntity` — read one scene entity's identity and pose.
//!
//! ## Why it lives here and not in `lunco-api`
//!
//! It used to be a hardcoded arm of `lunco-api`'s executor, which forced the
//! transport layer to know how to read a pose out of the ECS. It did that by
//! reading `GlobalTransform` — the RENDER frame, which big_space rebases onto the
//! floating origin — so a bolted-down prim reported a different position every
//! time the camera crossed a cell. A later implementation composed all the way
//! through the celestial root instead, which made a stationary surface object
//! drift as its body translated and rotated.
//!
//! The frame contract belongs to the crate that owns the scene verbs, so the read
//! side now sits beside the write side: `QueryEntity` reports exactly the active
//! physics frame `TransformEntity` accepts. Query a pose, hand it straight back,
//! and the object does not move. The concrete BigSpace grid remains an internal
//! implementation detail owned by `ActivePhysicsFrame`.
//!
//! The command uses the canonical API envelope:
//! `{"type":"ExecuteCommand","command":"QueryEntity","params":{"id":…}}`.

use bevy::ecs::query::QueryState;
use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::{CatalogEntryId, GlobalEntityId, UsdPrimKind};
use lunco_usd_bevy::UsdPrimPath;

/// `QueryEntity { id }` → that entity's name, kind, pose.
pub struct QueryEntityProvider;

impl ApiQueryProvider for QueryEntityProvider {
    fn name(&self) -> &'static str {
        "QueryEntity"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(raw) = params.get("id").and_then(serde_json::Value::as_u64) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "QueryEntity: `id` (entity id) required".to_string(),
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

        let Some(mut q_meta) = QueryState::<(
            Option<&Name>,
            Has<lunco_core::ControlBinding>,
            Option<&lunco_core::CelestialBody>,
            Option<&Transform>,
            Option<&CatalogEntryId>,
            Option<&UsdPrimKind>,
            Option<&UsdPrimPath>,
        )>::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryEntity: world state unavailable".to_string(),
            );
        };
        let Some(mut poses) = lunco_physics::SimulationPoseReadState::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryEntity: active physics frame unavailable".to_string(),
            );
        };

        let (name, accepts_commands, body, transform, catalog_id, usd_kind, prim_path) = q_meta
            .get(world, entity)
            .unwrap_or((None, false, None, None, None, None, None));
        let kind = usd_kind.map(|kind| kind.0.as_str()).unwrap_or("untyped");

        let Some((pos, rot)) = poses.pose(world, entity) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                format!("QueryEntity: entity {raw} is not connected to the active physics frame"),
            );
        };
        let pos = pos.0;
        let rot = rot.0.as_quat();
        // Object scale is authored on the object itself. Ancestor/grid scale is
        // not part of the rigid active-frame pose contract.
        let scale = transform.map_or(Vec3::ONE, |tf| tf.scale);
        // Euler YXZ (yaw, pitch, roll) — matches the sun / steering authoring
        // convention, handier than a quat.
        let (yaw, pitch, roll) = rot.to_euler(EulerRot::YXZ);
        ApiResponse::ok(serde_json::json!({
            "api_id": raw,
            "name": name.map(|n| n.as_str()).unwrap_or(""),
            "type": kind,
            "control_bound": accepts_commands,
            "celestial_body": body.is_some(),
            "catalog_id": catalog_id.map(|id| id.0.as_str()),
            "usd_prim_path": prim_path.map(|path| path.path.as_str()),
            "position": [pos.x, pos.y, pos.z],
            // The frame `position` is in, named on the wire: a client holding a
            // bare triple has no way to know whether it may hand it back.
            "position_frame": "active_physics",
            "rotation": [rot.x, rot.y, rot.z, rot.w],
            "euler": [yaw, pitch, roll],
            "scale": [scale.x, scale.y, scale.z],
        }))
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
    world.register_component::<lunco_core::ControlBinding>();
    world.register_component::<lunco_core::CelestialBody>();
    world.register_component::<Transform>();
    world.register_component::<CatalogEntryId>();
    world.register_component::<UsdPrimKind>();
    world.register_component::<UsdPrimPath>();
    world
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryEntityProvider);
}

#[cfg(test)]
mod tests {
    use super::*;
    use big_space::prelude::{CellCoord, Grid};

    fn query_test_app() -> App {
        let mut app = App::new();
        app.init_resource::<ApiEntityRegistry>();
        app.init_resource::<ApiQueryRegistry>();

        // Immutable QueryState construction cannot register component types on
        // demand. A production host registers these through its plugins; this
        // minimal fixture must establish the same component vocabulary before
        // the provider is invoked.
        let world = app.world_mut();
        world.register_component::<Name>();
        world.register_component::<lunco_core::ControlBinding>();
        world.register_component::<lunco_core::CelestialBody>();
        world.register_component::<Transform>();
        world.register_component::<CatalogEntryId>();
        world.register_component::<UsdPrimKind>();
        world.register_component::<UsdPrimPath>();
        world.register_component::<ChildOf>();
        world.register_component::<CellCoord>();
        world.register_component::<Grid>();
        world.register_component::<avian3d::prelude::RigidBody>();
        world.register_component::<avian3d::prelude::Position>();
        world.register_component::<avian3d::prelude::Rotation>();
        world.register_component::<lunco_physics::PhysicsPoseSeeded>();
        app
    }

    /// The round-trip contract: what `QueryEntity` reports is what `MoveEntity`
    /// takes. Pinned at a NON-zero cell, because in cell 0 the render frame, the
    /// cell-local `Transform` and the grid-absolute position are all equal and any
    /// frame bug hides — which is exactly why this shipped broken to the moonbase
    /// while the sandbox looked fine.
    #[test]
    fn reports_active_physics_position_not_the_render_frame() {
        const EDGE: f32 = 2000.0;
        let mut app = query_test_app();

        let grid = app
            .world_mut()
            .spawn((
                Grid::new(EDGE, 0.0),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut()
            .insert_resource(lunco_core::ActivePhysicsFrame(grid));
        // Two cells up, 53 m down within the cell: grid-absolute Y = 3947.
        let cell = CellCoord::new(0, 2, 0);
        let local = Vec3::new(10.0, -53.0, 4.0);
        let prim = app
            .world_mut()
            .spawn((
                Name::new("SolarPanel"),
                cell,
                Transform::from_translation(local),
                GlobalTransform::default(),
                ChildOf(grid),
            ))
            .id();
        let gid = GlobalEntityId::from_raw(42);
        app.world_mut()
            .resource_mut::<ApiEntityRegistry>()
            .assign(prim, gid);

        let response = QueryEntityProvider.execute(app.world(), &serde_json::json!({ "id": 42 }));
        let ApiResponse::Ok {
            data: Some(data), ..
        } = response
        else {
            panic!("expected a successful query, got {response:?}");
        };
        let pos = data["position"].as_array().expect("position array");
        let y = pos[1].as_f64().expect("numeric y");
        assert!(
            (y - (2.0 * EDGE as f64 - 53.0)).abs() < 1e-6,
            "position must be cell×edge + local (3947), got {y}"
        );
        assert_ne!(
            y, local.y as f64,
            "the cell-local translation must never pass for the position"
        );
        assert_eq!(data["position_frame"], "active_physics");
        assert_eq!(data["name"], "SolarPanel");
    }

    #[test]
    fn reports_rotation_in_the_same_active_physics_frame_as_position() {
        let mut app = query_test_app();

        let grid_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let grid = app
            .world_mut()
            .spawn((
                Grid::new(2_000.0, 0.0),
                CellCoord::ZERO,
                Transform::from_rotation(grid_rotation),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut()
            .insert_resource(lunco_core::ActivePhysicsFrame(grid));
        let local_rotation = Quat::from_rotation_x(0.3);
        let prim = app
            .world_mut()
            .spawn((
                CellCoord::ZERO,
                Transform::from_rotation(local_rotation),
                GlobalTransform::default(),
                ChildOf(grid),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ApiEntityRegistry>()
            .assign(prim, GlobalEntityId::from_raw(43));

        let ApiResponse::Ok {
            data: Some(data), ..
        } = QueryEntityProvider.execute(app.world(), &serde_json::json!({ "id": 43 }))
        else {
            panic!("expected successful query");
        };
        let q = data["rotation"].as_array().expect("rotation array");
        let reported = Quat::from_xyzw(
            q[0].as_f64().unwrap() as f32,
            q[1].as_f64().unwrap() as f32,
            q[2].as_f64().unwrap() as f32,
            q[3].as_f64().unwrap() as f32,
        );
        let expected = local_rotation;
        assert!(
            reported.dot(expected).abs() > 1.0 - 1e-6,
            "the active grid's own world rotation must not leak into its stable user frame: reported={reported:?} expected={expected:?}"
        );
    }

    #[test]
    fn stationary_surface_pose_does_not_follow_rotating_celestial_ancestors() {
        let mut app = query_test_app();
        let root = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                GlobalTransform::default(),
            ))
            .id();
        let body = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(80_000, 0, -20_000),
                Transform::from_rotation(Quat::from_rotation_y(0.4)),
                ChildOf(root),
            ))
            .id();
        let site = app
            .world_mut()
            .spawn((
                lunco_core::WorldGridConfig::default().grid(),
                CellCoord::new(0, -1, 0),
                Transform::from_rotation(Quat::from_rotation_x(-0.3)),
                ChildOf(body),
            ))
            .id();
        app.world_mut()
            .insert_resource(lunco_core::ActivePhysicsFrame(site));
        let rover = app
            .world_mut()
            .spawn((
                Name::new("Rover"),
                Transform::from_xyz(12.5, 98.0, -4.0),
                GlobalTransform::default(),
                ChildOf(site),
            ))
            .id();
        app.world_mut()
            .resource_mut::<ApiEntityRegistry>()
            .assign(rover, GlobalEntityId::from_raw(44));

        let read = |app: &mut App| {
            let ApiResponse::Ok {
                data: Some(data), ..
            } = QueryEntityProvider.execute(app.world(), &serde_json::json!({ "id": 44 }))
            else {
                panic!("expected successful query");
            };
            data["position"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_f64().unwrap())
                .collect::<Vec<_>>()
        };
        let before = read(&mut app);
        app.world_mut().entity_mut(body).insert((
            CellCoord::new(-120_000, 4_000, 90_000),
            Transform::from_rotation(Quat::from_rotation_z(-1.1)),
        ));
        let after = read(&mut app);

        assert_eq!(
            before, after,
            "surface coordinates must be body-motion invariant"
        );
        assert_eq!(after, vec![12.5, 98.0, -4.0]);
    }

    /// A missing entity is an error, not a silent (0,0,0).
    #[test]
    fn unknown_entity_is_an_error() {
        let app = query_test_app();
        let response = QueryEntityProvider.execute(app.world(), &serde_json::json!({ "id": 7 }));
        assert!(matches!(response, ApiResponse::Error { .. }));
    }
}
