//! Spatial and physics projections for scripting backends.
//!
//! This package owns the domain-specific pose, navigation, geolocation, and
//! entity-enumeration reads that sit on top of the language-neutral bridge.
//! Keeping these projections separate prevents Python or script-free bridge
//! users from compiling the celestial and physics graph merely to use generic
//! reflection and command/query access.

use bevy::ecs::system::SystemState;
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::*;
use lunco_api::registry::ApiEntityRegistry;
use lunco_celestial::CelestialBody;
use lunco_scripting_bridge_core::{ValueBuilder, resolve_entity, vec3_value, with_world};
use lunco_spatial::{
    NavigationCommand, SteeringGeometry,
    coords::{GridPos, VehicleFrame},
};

/// `world_pos(id)` — f64 position in the active simulation frame, or `None`.
///
/// For a surface scene this is the authored site frame used by Avian, terrain,
/// routes, and spawn commands. Celestial/root transforms are an implementation
/// detail and cannot leak into ordinary script navigation.
pub fn world_pos(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        state
            .get(world)
            .ok()?
            .position(entity)
            .map(|position| position.0)
    })
    .flatten()
}

/// `geolocation(id)` — where on the body an entity actually is, as
/// `(lat_deg, lon_deg, height_m)`. `None` when the scene is not site-anchored
/// (no `SiteAnchor`) or the anchor's body is not present.
///
/// Works for any positioned entity — route point, mast, marker — through
/// the same explicit site/body-fixed frame query used by HUDs and billboards.
/// Root-world position is deliberately not a fallback: celestial ancestors
/// move with ephemeris time and are not site ENU coordinates.
pub fn geolocation(gid: u64) -> Option<lunco_celestial::Geodetic> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
            Query<(Entity, &lunco_celestial::GeodeticAnchor), With<lunco_celestial::SiteAnchor>>,
            Res<lunco_celestial::CelestialBodyRegistry>,
            Res<lunco_celestial_spatial_core::ReferenceFrameIndex>,
        )> = SystemState::new(world);
        let (q_parents, q_grids, q_spatial, q_site, bodies, frame_index) = state.get(world).ok()?;
        lunco_celestial_spatial_core::resolve_surface_pose(
            entity,
            &q_site,
            &bodies,
            &frame_index,
            &q_parents,
            &q_grids,
            &q_spatial,
        )
        .map(|pose| pose.geodetic)
    })
    .flatten()
}

/// `world_forward(id)` — unit heading in the active simulation frame, or `None`.
pub fn world_forward(gid: u64) -> Option<DVec3> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        let rotation = state.get(world).ok()?.rotation(entity)?;
        Some(rotation.0 * DVec3::NEG_Z)
    })
    .flatten()
}

/// Compute one navigation command through the shared core law for a live vessel.
/// This is the language-neutral scripting seam: Rhai and any future backend
/// pass the same target/speed/radius contract and receive the same authored
/// steering-capability result. Missing pose, geometry, or invalid input is a
/// miss; callers must hold the vehicle brake rather than invent a fallback.
pub fn navigation_command(
    vessel_gid: u64,
    target: DVec3,
    speed: f64,
    radius: f32,
) -> Option<NavigationCommand> {
    with_world(|world| {
        let entity = resolve_entity(world, vessel_gid)?;
        let geometry = *world.get::<SteeringGeometry>(entity)?;
        let (pos, rotation) = {
            let mut pose: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
            pose.get(world).ok()?.pose(entity)?
        };
        let target = GridPos(target);
        let fwd = VehicleFrame::forward(rotation).as_vec3();
        lunco_spatial::nav_setpoint(pos, fwd, target, speed, radius, geometry)
    })
    .flatten()
}

/// `world_rotation(id)` — orientation in the active simulation frame as a
/// quaternion `[x, y, z, w]`, or `None`. The GENERAL orientation accessor: every axis
/// (`up`, `forward`, `right`) is `quat * unit_axis`, derived rhai-side, so this
/// one host fn subsumes `world_forward` and unblocks tilt/tip-over logic (a rover
/// is tipped when its up-vector's `y` drops below `cos(θ)`) without a per-axis
/// Rust fn each. It uses the same active-frame hierarchy sample as
/// `world_forward`, so surface-up remains +Y below a rotated celestial branch.
pub fn world_rotation(gid: u64) -> Option<[f64; 4]> {
    world_rotation_quat(gid).map(|q| [q.x, q.y, q.z, q.w])
}

/// Native counterpart of [`world_rotation`]. Backends that can retain glam
/// values should use this path so the pose query does not lower to an array and
/// immediately reconstruct the same quaternion. Invalid/non-finite samples are
/// unavailable rather than entering a control calculation.
pub fn world_rotation_quat(gid: u64) -> Option<DQuat> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<lunco_physics::SimulationPoseQuery> = SystemState::new(world);
        let q = state.get(world).ok()?.rotation(entity)?.0;
        (q.is_finite() && q.length_squared() >= 1.0e-24).then_some(q.normalize())
    })
    .flatten()
}

/// `viewport_position(id)` — project a live render entity into the active
/// scene viewport's logical pixel coordinates, or `None` when the entity,
/// viewport, camera, or projection is unavailable.
///
/// This is a presentation query, not a simulation-frame conversion. It uses
/// the entity's propagated `GlobalTransform` and the explicitly active
/// `SceneCamera`, so a script that drives typed pointer events can address the
/// same visual object the operator sees after camera motion or floating-origin
/// rebasing. The caller owns the interaction policy; this function only
/// exposes Bevy's canonical world-to-viewport projection.
pub fn viewport_position(gid: u64) -> Option<Vec2> {
    with_world(|world| {
        let entity = resolve_entity(world, gid)?;
        let mut state: SystemState<(
            Query<&GlobalTransform>,
            Query<(&Camera, &GlobalTransform), (With<Camera3d>, With<lunco_render::SceneCamera>)>,
            Res<lunco_viewport_core::SceneViewport>,
        )> = SystemState::new(world);
        let (q_transforms, q_cameras, scene_viewport) = state.get(world).ok()?;
        let camera_entity = scene_viewport.active_camera?;
        let (camera, camera_transform) = q_cameras.get(camera_entity).ok()?;
        if !camera.is_active {
            return None;
        }
        let position = q_transforms.get(entity).ok()?.translation();
        camera.world_to_viewport(camera_transform, position).ok()
    })
    .flatten()
}

/// `list_entities()` — `[{ id, name, type, pos, catalog_id, input_surface,
/// control_bound, celestial_body }]` for every registered entity. `type` comes
/// from the projected USD `kind`; it is never inferred from control or physics
/// components. `catalog_id` is present only for catalog-spawned entities, and
/// `input_surface` is the authoritative `InputPorts` readiness bit. `name` is
/// the shared human-readable label; the full USD path remains available through
/// the USD bridge for callers that need canonical addressing.
pub fn list_entities<B: ValueBuilder>(b: &B) -> B::Value {
    with_world(|world| {
        let Some(pairs) = world
            .get_resource::<ApiEntityRegistry>()
            .map(ApiEntityRegistry::entities)
        else {
            return b.array(Vec::new());
        };
        // One SystemState carries every per-entity read so the loop never
        // re-borrows the World.
        let mut state: SystemState<(
            lunco_physics::SimulationPoseQuery,
            Query<(
                Option<&Name>,
                Option<&lunco_core::markers::Callsign>,
                Has<lunco_control_core::ControlBinding>,
                Has<lunco_port_core::InputPorts>,
                Option<&CelestialBody>,
                Option<&lunco_core::CatalogEntryId>,
                Option<&lunco_core::UsdPrimKind>,
            )>,
        )> = SystemState::new(world);
        let Some((poses, q_meta)) = state.get(world).ok() else {
            return b.array(Vec::new());
        };
        let items = pairs
            .into_iter()
            .map(|(gid, entity)| {
                let (name, callsign, accepts_commands, input_surface, body, catalog_id, usd_kind) =
                    q_meta
                        .get(entity)
                        .unwrap_or((None, None, false, false, None, None, None));
                let kind = usd_kind.map(|kind| kind.0.as_str()).unwrap_or("untyped");
                let pos = poses
                    .position(entity)
                    .map(|v| vec3_value(b, v.0.x, v.0.y, v.0.z))
                    .unwrap_or_else(|| b.unit());
                b.map(vec![
                    ("id".to_string(), b.int(gid.get() as i64)),
                    (
                        "name".to_string(),
                        b.string(&lunco_core::entity_display_name(name, callsign, catalog_id)),
                    ),
                    ("type".to_string(), b.string(kind)),
                    ("input_surface".to_string(), b.bool(input_surface)),
                    ("control_bound".to_string(), b.bool(accepts_commands)),
                    ("celestial_body".to_string(), b.bool(body.is_some())),
                    (
                        "catalog_id".to_string(),
                        b.string(catalog_id.map(|id| id.0.as_str()).unwrap_or("")),
                    ),
                    ("pos".to_string(), pos),
                ])
            })
            .collect();
        b.array(items)
    })
    .unwrap_or_else(|| b.array(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_core::GlobalEntityId;

    #[test]
    fn script_pose_reads_are_empty_before_a_simulation_frame_exists() {
        let mut world = World::new();
        world.init_resource::<ApiEntityRegistry>();
        let entity = world.spawn_empty().id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(entity, GlobalEntityId::from_raw(42));

        let _scope = lunco_scripting_bridge_core::WorldScope::enter(&mut world);
        assert_eq!(world_pos(42), None);
        assert_eq!(world_forward(42), None);
        assert_eq!(world_rotation(42), None);
    }

    #[test]
    fn script_pose_reads_share_the_active_frame_below_rotating_ancestors() {
        let mut world = World::new();
        let world_grid = lunco_spatial::ensure_world_root(&mut world);
        world.insert_resource(lunco_spatial::ActivePhysicsFrame(world_grid));
        world.init_resource::<ApiEntityRegistry>();
        let root = world.resource::<lunco_spatial::ActivePhysicsFrame>().0;
        let body = world
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::new(100_000, -2_000, 40_000),
                Transform::from_rotation(Quat::from_rotation_x(0.9)),
                ChildOf(root),
            ))
            .id();
        let site = world
            .spawn((
                lunco_spatial::WorldGridConfig::default().grid(),
                CellCoord::new(800, -950, 300),
                Transform::from_rotation(Quat::from_rotation_z(-0.7)),
                ChildOf(body),
            ))
            .id();
        world.insert_resource(lunco_spatial::ActivePhysicsFrame(site));
        let local_position = DVec3::new(14.0, -1_901.5, -8.0);
        let local_rotation = DQuat::from_rotation_y(0.35);
        let entity = world
            .spawn((
                Transform::from_translation(local_position.as_vec3())
                    .with_rotation(local_rotation.as_quat()),
                ChildOf(site),
            ))
            .id();
        world
            .resource_mut::<ApiEntityRegistry>()
            .assign(entity, GlobalEntityId::from_raw(42));

        let _scope = lunco_scripting_bridge_core::WorldScope::enter(&mut world);
        let position = world_pos(42).expect("position");
        let forward = world_forward(42).expect("forward");
        let rotation = world_rotation(42).expect("rotation");
        let native_rotation = world_rotation_quat(42).expect("native rotation");

        assert!((position - local_position).length() < 1.0e-4);
        assert!((forward - local_rotation * DVec3::NEG_Z).length() < 1.0e-6);
        let rotation = DQuat::from_array(rotation);
        assert!(rotation.angle_between(local_rotation).abs() < 1.0e-6);
        assert!(native_rotation.angle_between(local_rotation).abs() < 1.0e-6);
    }
}
