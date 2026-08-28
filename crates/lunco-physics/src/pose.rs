//! Authoritative entity poses in the active simulation frame.
//!
//! A physical body's pose has one owner: Avian's f64 [`Position`] and
//! [`Rotation`]. Its cell-local [`Transform`] is only the render projection and
//! necessarily rounds to f32. Non-physical entities have no Avian state, so
//! their authoritative pose is the composed BigSpace hierarchy exposed by
//! [`ActiveFramePoseQuery`].
//!
//! [`SimulationPoseQuery`] is the single read boundary that applies that rule.
//! Callers never "prefer Position when present": an Avian body without
//! [`PhysicsPoseSeeded`] is not ready and returns `None`, rather than silently
//! reporting Avian's required-component default or falling back to rounded
//! presentation state.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::ecs::query::QueryState;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_core::coords::{pose_in_grid, ActiveFramePoseQuery, GridPos, GridRot};

/// Read-only query state for API and other non-system callers.
///
/// Bevy's [`SystemState`] is initialized from `&mut World`, which is correct
/// for systems but incompatible with the API query contract. This state uses
/// Bevy's immutable-world [`QueryState::try_new`] path and shares the
/// canonical [`pose_in_grid`] conversion with [`SimulationPoseQuery`].
pub struct SimulationPoseReadState {
    bodies: QueryState<(), With<RigidBody>>,
    physics: QueryState<
        (&'static Position, &'static Rotation),
        (With<RigidBody>, With<PhysicsPoseSeeded>),
    >,
    parents: QueryState<&'static ChildOf>,
    grids: QueryState<&'static Grid>,
    spatial: QueryState<(Option<&'static CellCoord>, &'static Transform)>,
}

impl SimulationPoseReadState {
    /// Build the query state without requiring mutable access to the world.
    pub fn try_new(world: &World) -> Option<Self> {
        Some(Self {
            bodies: QueryState::try_new(world)?,
            physics: QueryState::try_new(world)?,
            parents: QueryState::try_new(world)?,
            grids: QueryState::try_new(world)?,
            spatial: QueryState::try_new(world)?,
        })
    }

    /// Resolve one entity in the active physics frame.
    pub fn pose(&mut self, world: &World, entity: Entity) -> Option<(GridPos, GridRot)> {
        let frame = world.get_resource::<lunco_core::ActivePhysicsFrame>()?.0;
        if self.bodies.get(world, entity).is_ok() {
            let (position, rotation) = self.physics.get(world, entity).ok()?;
            return Some((GridPos(position.0), GridRot(rotation.0)));
        }

        let parents = self.parents.query(world);
        let grids = self.grids.query(world);
        let spatial = self.spatial.query(world);
        pose_in_grid(entity, frame, &parents, &grids, &spatial)
            .map(|(position, rotation)| (GridPos(position), GridRot(rotation)))
    }

    /// Resolve only an entity's active-frame position.
    pub fn position(&mut self, world: &World, entity: Entity) -> Option<GridPos> {
        self.pose(world, entity).map(|(position, _)| position)
    }
}

/// The BigSpace/Avian bridge has initialized this body's f64 physics pose.
///
/// This is readiness metadata, not another pose store. The bridge inserts it
/// only after writing a real active-frame [`Position`]/[`Rotation`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct PhysicsPoseSeeded;

/// Read any entity in the active simulation frame from its authoritative owner.
#[derive(SystemParam)]
pub struct SimulationPoseQuery<'w, 's> {
    hierarchy: ActiveFramePoseQuery<'w, 's>,
    bodies: Query<'w, 's, (), With<RigidBody>>,
    physics: Query<
        'w,
        's,
        (&'static Position, &'static Rotation),
        (With<RigidBody>, With<PhysicsPoseSeeded>),
    >,
}

impl SimulationPoseQuery<'_, '_> {
    /// Resolve `entity` into the active simulation frame.
    ///
    /// A physical body that has not been seeded returns `None`; rounded render
    /// state is never substituted for missing authoritative state.
    pub fn pose(&self, entity: Entity) -> Option<(GridPos, GridRot)> {
        if self.bodies.contains(entity) {
            let (position, rotation) = self.physics.get(entity).ok()?;
            return Some((GridPos(position.0), GridRot(rotation.0)));
        }
        self.hierarchy.pose(entity)
    }

    pub fn position(&self, entity: Entity) -> Option<GridPos> {
        self.pose(entity).map(|(position, _)| position)
    }

    pub fn rotation(&self, entity: Entity) -> Option<GridRot> {
        self.pose(entity).map(|(_, rotation)| rotation)
    }
}

fn parse_point(params: &serde_json::Value, key: &str) -> Option<bevy::math::DVec3> {
    let values = params.get(key)?.as_array()?;
    if values.len() != 3 {
        return None;
    }
    Some(bevy::math::DVec3::new(
        values[0].as_f64()?,
        values[1].as_f64()?,
        values[2].as_f64()?,
    ))
}

/// Closest registered entity to an active-frame point.
pub struct NearestProvider;

impl ApiQueryProvider for NearestProvider {
    fn name(&self) -> &'static str {
        "Nearest"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(point) = parse_point(params, "point") else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "Nearest: `point` [x,y,z] required".to_string(),
            );
        };
        let max = params.get("max").and_then(serde_json::Value::as_f64);
        let exclude = params.get("exclude").and_then(serde_json::Value::as_u64);
        let entities = world.resource::<ApiEntityRegistry>().entities();
        let Some(mut poses) = SimulationPoseReadState::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "Nearest: simulation pose query is unavailable".to_string(),
            );
        };
        let mut best: Option<(u64, f64, bevy::math::DVec3)> = None;
        for (gid, entity) in entities {
            if exclude == Some(gid.get()) {
                continue;
            }
            let Some(position) = poses.position(world, entity) else {
                continue;
            };
            let distance = position.0.distance(point);
            if max.is_some_and(|limit| distance > limit) {
                continue;
            }
            if best.as_ref().is_none_or(|current| distance < current.1) {
                best = Some((gid.get(), distance, position.0));
            }
        }
        match best {
            Some((id, distance, position)) => ApiResponse::ok(serde_json::json!({
                "id": id,
                "distance": distance,
                "point": [position.x, position.y, position.z]
            })),
            None => ApiResponse::ok(serde_json::json!({ "id": serde_json::Value::Null })),
        }
    }
}

/// Every registered entity within `radius` of an active-frame point.
pub struct EntitiesInRadiusProvider;

impl ApiQueryProvider for EntitiesInRadiusProvider {
    fn name(&self) -> &'static str {
        "EntitiesInRadius"
    }

    fn execute(&self, world: &World, params: &serde_json::Value) -> ApiResponse {
        let Some(point) = parse_point(params, "point") else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "EntitiesInRadius: `point` [x,y,z] required".to_string(),
            );
        };
        let radius = params
            .get("radius")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let exclude = params.get("exclude").and_then(serde_json::Value::as_u64);
        let entities = world.resource::<ApiEntityRegistry>().entities();
        let Some(mut poses) = SimulationPoseReadState::try_new(world) else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "EntitiesInRadius: simulation pose query is unavailable".to_string(),
            );
        };
        let ids: Vec<u64> = entities
            .into_iter()
            .filter(|(gid, _)| exclude != Some(gid.get()))
            .filter_map(|(gid, entity)| {
                (poses.position(world, entity)?.0.distance(point) <= radius).then_some(gid.get())
            })
            .collect();
        ApiResponse::ok(serde_json::json!({ "count": ids.len(), "ids": ids }))
    }
}

/// Register active-frame spatial reads beside the physics gate.
pub fn register_spatial_query_providers(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    let mut registry = app.world_mut().resource_mut::<ApiQueryRegistry>();
    registry.register(NearestProvider);
    registry.register(EntitiesInRadiusProvider);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::SystemState;
    use bevy::math::{DQuat, DVec3};

    fn read(world: &mut World, entity: Entity) -> Option<(GridPos, GridRot)> {
        let mut state: SystemState<SimulationPoseQuery> = SystemState::new(world);
        state.get(world).expect("query validates").pose(entity)
    }

    #[test]
    fn physical_body_uses_exact_f64_pose_not_rounded_transform() {
        let mut world = World::new();
        let frame = world
            .spawn(lunco_core::WorldGridConfig::default().grid())
            .id();
        world.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        let exact = DVec3::new(0.123_456_789_012, -1_901.623_456_789, 4.987_654_321_098);
        let exact_rotation = DQuat::from_rotation_y(0.123_456_789);
        let body = world
            .spawn((
                RigidBody::Dynamic,
                Position(exact),
                Rotation(exact_rotation),
                PhysicsPoseSeeded,
                Transform::from_translation(exact.as_vec3()),
                ChildOf(frame),
            ))
            .id();

        let (position, rotation) = read(&mut world, body).expect("seeded body pose");
        assert_eq!(position.0, exact);
        assert_eq!(rotation.0, exact_rotation);
        assert_ne!(position.0.y, exact.y as f32 as f64);
    }

    #[test]
    fn unseeded_physical_body_has_no_reportable_pose() {
        let mut world = World::new();
        let frame = world
            .spawn(lunco_core::WorldGridConfig::default().grid())
            .id();
        world.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        let body = world
            .spawn((
                RigidBody::Dynamic,
                Position(DVec3::ZERO),
                Rotation(DQuat::IDENTITY),
                Transform::from_xyz(12.0, 34.0, 56.0),
                ChildOf(frame),
            ))
            .id();

        assert!(read(&mut world, body).is_none());
    }

    #[test]
    fn non_physical_entity_uses_bigspace_hierarchy() {
        let mut world = World::new();
        let frame = world
            .spawn(lunco_core::WorldGridConfig::default().grid())
            .id();
        world.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        let marker = world
            .spawn((Transform::from_xyz(12.0, -34.0, 56.0), ChildOf(frame)))
            .id();

        let (position, _) = read(&mut world, marker).expect("hierarchy pose");
        assert_eq!(position.0, DVec3::new(12.0, -34.0, 56.0));
    }

    #[test]
    fn proximity_provider_uses_exact_physics_position() {
        let mut world = World::new();
        let frame = world
            .spawn(lunco_core::WorldGridConfig::default().grid())
            .id();
        world.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        world.register_component::<CellCoord>();
        world.init_resource::<ApiEntityRegistry>();
        let exact = DVec3::new(12.123_456_789, -1_900.987_654_321, -4.0);
        let body = world
            .spawn((
                RigidBody::Dynamic,
                Position(exact),
                Rotation(DQuat::IDENTITY),
                PhysicsPoseSeeded,
                Transform::from_translation(exact.as_vec3()),
                ChildOf(frame),
            ))
            .id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(body, lunco_core::GlobalEntityId::from_raw(42));

        let response = NearestProvider.execute(
            &mut world,
            &serde_json::json!({"point": [12.0, -1901.0, -4.0]}),
        );
        let ApiResponse::Ok {
            data: Some(data), ..
        } = response
        else {
            panic!("nearest query did not return data");
        };
        assert_eq!(
            data["point"],
            serde_json::json!([exact.x, exact.y, exact.z])
        );
    }
}
