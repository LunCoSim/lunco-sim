//! `QueryEntity` — read one scene entity's identity and pose.
//!
//! ## Why it lives here and not in `lunco-api`
//!
//! It used to be a hardcoded arm of `lunco-api`'s executor, which forced the
//! transport layer to know how to read a pose out of the ECS. It did that by
//! reading `GlobalTransform` — the RENDER frame, which big_space rebases onto the
//! floating origin — so a bolted-down prim reported a different position every
//! time the camera crossed a cell, and the number matched neither the authored
//! USD nor what [`MoveEntity`](crate::commands::MoveEntity) takes back.
//!
//! The frame contract belongs to the crate that owns the scene verbs, so the read
//! side now sits beside the write side: `QueryEntity` reports exactly the
//! grid-absolute frame `MoveEntity` accepts. Query a position, hand it straight
//! back, and the object does not move — a property this pair could not have had
//! while they lived in different crates with different ideas about frames.
//!
//! The wire is unchanged: `{"type":"QueryEntity","id":…}` still works, because
//! `lunco-api`'s envelope maps that shape onto this provider.

use bevy::prelude::*;
use bevy::ecs::system::SystemState;
use big_space::prelude::{CellCoord, Grid};
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::GlobalEntityId;

/// `QueryEntity { id }` → that entity's name, kind, pose.
pub struct QueryEntityProvider;

impl ApiQueryProvider for QueryEntityProvider {
    fn name(&self) -> &'static str {
        "QueryEntity"
    }

    fn execute(&self, world: &mut World, params: &serde_json::Value) -> ApiResponse {
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

        let mut state: SystemState<(
            Query<(
                Option<&Name>,
                Has<lunco_core::ControlBinding>,
                Option<&lunco_core::CelestialBody>,
            )>,
            Query<&GlobalTransform>,
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(world);
        let Ok((q_meta, q_gt, q_parents, q_grids, q_spatial)) = state.get(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "QueryEntity: world state unavailable".to_string(),
            );
        };

        let (name, accepts_commands, body) = q_meta.get(entity).unwrap_or((None, false, None));
        // NOTE: the reported kind string is deliberately unchanged — a lander accepts
        // commands and has always reported as "rover" here.
        let kind = if accepts_commands {
            "rover"
        } else if body.is_some() {
            "planet"
        } else {
            "unknown"
        };

        // Orientation/scale from `GlobalTransform`: a rotation is the same in the
        // render frame and the grid's (big_space rebases the origin, it does not
        // spin it), so this is the cheap correct source for them.
        let (scale, rot, _) = q_gt
            .get(entity)
            .ok()
            .map(|gt| gt.to_scale_rotation_translation())
            .unwrap_or((Vec3::ONE, Quat::IDENTITY, Vec3::ZERO));
        // Position is frame-sensitive, so it comes off the cell chain instead.
        let pos = lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial)
            .unwrap_or(bevy::math::DVec3::ZERO);
        // Euler YXZ (yaw, pitch, roll) — matches the sun / steering authoring
        // convention, handier than a quat.
        let (yaw, pitch, roll) = rot.to_euler(EulerRot::YXZ);

        ApiResponse::ok(serde_json::json!({
            "api_id": raw,
            "name": name.map(|n| n.as_str()).unwrap_or(""),
            "type": kind,
            "position": [pos.x, pos.y, pos.z],
            // The frame `position` is in, named on the wire: a client holding a
            // bare triple has no way to know whether it may hand it back.
            "position_frame": "grid_absolute",
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
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(QueryEntityProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The round-trip contract: what `QueryEntity` reports is what `MoveEntity`
    /// takes. Pinned at a NON-zero cell, because in cell 0 the render frame, the
    /// cell-local `Transform` and the grid-absolute position are all equal and any
    /// frame bug hides — which is exactly why this shipped broken to the moonbase
    /// while the sandbox looked fine.
    #[test]
    fn reports_grid_absolute_not_the_render_frame() {
        const EDGE: f32 = 2000.0;
        let mut app = App::new();
        app.init_resource::<ApiEntityRegistry>();
        app.init_resource::<ApiQueryRegistry>();

        let grid = app
            .world_mut()
            .spawn((Grid::new(EDGE, 0.0), CellCoord::ZERO, Transform::default(), GlobalTransform::default()))
            .id();
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

        let response = QueryEntityProvider.execute(
            app.world_mut(),
            &serde_json::json!({ "id": 42 }),
        );
        let ApiResponse::Ok { data: Some(data), .. } = response else {
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
        assert_eq!(data["position_frame"], "grid_absolute");
        assert_eq!(data["name"], "SolarPanel");
    }

    /// A missing entity is an error, not a silent (0,0,0).
    #[test]
    fn unknown_entity_is_an_error() {
        let mut app = App::new();
        app.init_resource::<ApiEntityRegistry>();
        let response =
            QueryEntityProvider.execute(app.world_mut(), &serde_json::json!({ "id": 7 }));
        assert!(matches!(response, ApiResponse::Error { .. }));
    }
}
