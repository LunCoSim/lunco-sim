//! Keyed live visual markers and optional overlap sensors parented to a mounted scene prim.
use crate::runtime_objects::{commit_sequence, derived_gid, next_sequence, valid_key};
use avian3d::prelude::*;
use bevy::{
    ecs::system::SystemState,
    math::{DQuat, DVec3},
    prelude::*,
};
use big_space::prelude::{CellCoord, Grid};
use lunco_api::{
    executor::{CommandReply, DeferredCommandAppExt},
    queries::{
        ApiQueryProvider, ApiQueryRegistry, SimulationQueryReadScope, api_param_str, api_param_u64,
    },
    registry::ApiEntityRegistry,
};
use lunco_api_core::{ApiErrorCode, ApiQueryParameterSchema, ApiQuerySchema, ApiValue, api_value};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{ActiveCommandId, GlobalEntityId, SceneMountState, on_command};
use lunco_core_session::{NetworkRole, SkipContentStamp};
use lunco_interaction_core::{PointerInteraction, ScenePointerPolicy};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_scene_command_contracts::SetRuntimeMarker;
use lunco_usd_bevy_scene::{NativeViewVisibility, UsdPrimPath};

#[derive(Clone, Copy)]
struct Part {
    entity: Entity,
    gid: GlobalEntityId,
}

#[derive(Component, Clone)]
struct RuntimeMarker {
    key: String,
    actor: Option<GlobalEntityId>,
    visual: Part,
    sensor: Option<Part>,
    radius: f64,
    color: [f32; 3],
    opacity: f32,
    sensor_radius: Option<f64>,
}

impl RuntimeMarker {
    fn record(&self, gid: GlobalEntityId, center: [f64; 3]) -> ApiValue {
        api_value!({
            "marker_id": gid.get(), "visual_id": self.visual.gid.get(),
            "sensor_id": self.sensor.map(|part| part.gid.get()), "center": center,
            "radius": self.radius, "color": self.color, "opacity": self.opacity,
            "sensor_radius": self.sensor_radius,
        })
    }
}

/// Marker roots and parts are owned by the marker command, not generic pose or delete operations.
#[derive(Component)]
pub(crate) struct RuntimeMarkerPart;

/// Shared immutable geometry. Marker dimensions belong to individual visual children.
#[derive(Resource)]
struct MarkerMesh(Handle<Mesh>);

fn validate_owner(world: &World, owner: Entity) -> Result<GlobalEntityId, String> {
    let prim = world
        .get::<UsdPrimPath>(owner)
        .ok_or("Runtime marker owner must be a mounted USD prim")?;
    let root = lunco_usd_bevy_scene::scene_root_in_world(world, owner)
        .map_err(|err| format!("Invalid marker hierarchy: {err:?}"))?;
    if !root.is_some_and(|root| {
        world
            .get_resource::<SceneMountState>()
            .is_some_and(|mount| mount.contains_root(root))
    }) || !lunco_usd_bevy_scene::is_mounted_scene_stage(world, prim.stage_handle.id())
        || lunco_usd_bevy_scene::is_preview_only_entity(world, owner)
    {
        return Err("Runtime marker owner is outside the mounted scene".into());
    }
    world
        .get::<GlobalEntityId>(owner)
        .copied()
        .ok_or_else(|| "Runtime marker owner has no stable identity".into())
}

fn markers(world: &World, owner: Entity) -> impl Iterator<Item = (Entity, &RuntimeMarker)> {
    world
        .get::<Children>(owner)
        .into_iter()
        .flatten()
        .filter_map(|&e| world.get::<RuntimeMarker>(e).map(|marker| (e, marker)))
}

fn backend_radius(radius: f64) -> Result<f32, String> {
    // Bevy mesh scale and the native collider shape both require f32.
    let value = radius as f32;
    if !value.is_finite() || value <= 0.0 {
        return Err(
            "Marker radii must be positive and fit the rendering and collider backends".into(),
        );
    }
    Ok(value)
}

fn marker_pose(
    world: &mut World,
    parent: Entity,
    position: [f64; 3],
) -> Result<(Option<CellCoord>, Transform), String> {
    let position = DVec3::from_array(position);
    if !position.is_finite() {
        return Err("Marker center must be finite".into());
    }
    let frame = world
        .get_resource::<lunco_spatial::ActivePhysicsFrame>()
        .ok_or("Active physics frame unavailable")?
        .0;
    let mut reads: SystemState<(
        Query<&ChildOf>,
        Query<&Grid>,
        Query<(Option<&CellCoord>, &Transform)>,
    )> = SystemState::new(world);
    let (parents, grids, poses) = reads.get(world).map_err(|error| error.to_string())?;
    let (local_position, rotation) = lunco_spatial::coords::pose_in_parent_local(
        position,
        DQuat::IDENTITY,
        parent,
        frame,
        &parents,
        &grids,
        &poses,
    )
    .ok_or("Marker owner is disconnected from the active physics frame")?;
    let (cell, translation) =
        crate::commands::checked_pose_storage(grids.get(parent).ok(), local_position)
            .map_err(|error| error.to_string())?;
    let rotation = rotation.as_quat();
    if !translation.is_finite() || !rotation.is_finite() {
        return Err("Marker pose exceeds parent storage range".into());
    }
    Ok((
        cell,
        Transform::from_translation(translation).with_rotation(rotation),
    ))
}

fn remove_marker(world: &mut World, root: Entity, marker: &RuntimeMarker) {
    if let Some(mut selected) = world.get_resource_mut::<lunco_scene_selection::SelectedEntities>()
    {
        selected.entities.retain(|&e| {
            e != root
                && e != marker.visual.entity
                && !marker.sensor.is_some_and(|part| e == part.entity)
        });
    }
    world.despawn(root);
}

fn set_marker(
    world: &mut World,
    cmd: SetRuntimeMarker,
    actor: Option<GlobalEntityId>,
) -> Result<Ack, String> {
    if world
        .get_resource::<NetworkRole>()
        .is_none_or(|role| role.is_networked())
    {
        return Err("Runtime markers require a standalone scene".into());
    }
    let parent_gid = validate_owner(world, cmd.owner)?;
    if !valid_key(&cmd.key) {
        return Err("Invalid runtime marker key".into());
    }
    let existing = markers(world, cmd.owner)
        .find(|(_, marker)| marker.key == cmd.key)
        .map(|(entity, marker)| (entity, marker.clone()));
    if existing
        .as_ref()
        .is_some_and(|(_, marker)| marker.actor != actor)
    {
        return Err("Runtime marker belongs to a different scenario or command owner".into());
    }
    let Some(spec) = cmd.marker else {
        if let Some((entity, marker)) = existing {
            remove_marker(world, entity, &marker);
        }
        return Ok(Ack::with_data(OpId::new(), api_value!({"marker": ()})));
    };
    let radius = backend_radius(spec.radius)?;
    let sensor_radius = spec.sensor_radius.map(backend_radius).transpose()?;
    if let Some(r) = sensor_radius {
        let center = DVec3::from_array(spec.center);
        let half = DVec3::splat(f64::from(r));
        if !lunco_physics::avian_backend_aabb_is_valid(center - half, center + half) {
            return Err("Sensor bounds exceed the active physics frame".into());
        }
    }
    if spec.color.iter().any(|v| !v.is_finite() || *v < 0.0) || !(0.0..=1.0).contains(&spec.opacity)
    {
        return Err("Invalid linear marker color or opacity".into());
    }
    let (cell, transform) = marker_pose(world, cmd.owner, spec.center)?;
    if world.get_resource::<Assets<Mesh>>().is_none() {
        return Err("Native marker meshes unavailable".into());
    }
    let sequence = next_sequence(world)?;
    let registry = world.resource::<ApiEntityRegistry>();
    let allocate = |part: &str| {
        derived_gid(
            parent_gid.get(),
            format!("marker/{}/{sequence}/{part}", cmd.key),
            registry,
        )
    };
    let (marker_gid, visual_gid, existing_sensor) = if let Some((root, marker)) = &existing {
        for child in [Some(marker.visual), marker.sensor].into_iter().flatten() {
            if world.get::<ChildOf>(child.entity).map(ChildOf::parent) != Some(*root)
                || world.get::<GlobalEntityId>(child.entity) != Some(&child.gid)
            {
                return Err(format!(
                    "Incomplete runtime marker hierarchy for {}",
                    cmd.key
                ));
            }
        }
        (
            *world
                .get::<GlobalEntityId>(*root)
                .ok_or("Runtime marker identity missing")?,
            marker.visual.gid,
            marker.sensor,
        )
    } else {
        (allocate("marker")?, allocate("visual")?, None)
    };
    let sensor_gid = if spec.sensor_radius.is_some() {
        Some(match existing_sensor {
            Some(part) => part.gid,
            None => allocate("sensor")?,
        })
    } else {
        None
    };
    if marker_gid == visual_gid || sensor_gid == Some(marker_gid) || sensor_gid == Some(visual_gid)
    {
        return Err("Runtime marker identities collide".into());
    }
    enum SensorChange {
        Keep(Option<Part>),
        Add(GlobalEntityId, Collider),
        Resize(Part, Collider),
        Remove(Part),
    }
    let sensor_change = match (existing_sensor, sensor_radius, sensor_gid) {
        (Some(part), Some(_), Some(_))
            if existing.as_ref().and_then(|(_, m)| m.sensor_radius) == spec.sensor_radius =>
        {
            SensorChange::Keep(Some(part))
        }
        (part, Some(radius), Some(gid)) => {
            let shape = Collider::sphere(radius);
            if !lunco_physics::avian_backend_collider_shape_is_valid(&shape) {
                return Err("Sensor radius exceeds native collider bounds".into());
            }
            match part {
                Some(part) => SensorChange::Resize(part, shape),
                None => SensorChange::Add(gid, shape),
            }
        }
        (Some(part), None, None) => SensorChange::Remove(part),
        (None, None, None) => SensorChange::Keep(None),
        _ => return Err("Inconsistent runtime marker sensor identity".into()),
    };
    // All validation precedes publication. The next physics step consumes the
    // final hierarchy and its native sensor components in command submission order.
    if existing.is_none() || (existing_sensor.is_none() && sensor_gid.is_some()) {
        commit_sequence(world, sequence);
    }
    let pointer = ScenePointerPolicy {
        left: PointerInteraction::PassThrough,
        right: PointerInteraction::Context,
    };
    let (root, visual) = match &existing {
        Some((entity, marker)) => (*entity, marker.visual),
        None => {
            let mesh = if let Some(mesh) = world.get_resource::<MarkerMesh>() {
                mesh.0.clone()
            } else {
                let mesh = world.resource_mut::<Assets<Mesh>>().add(Sphere::new(1.0));
                world.insert_resource(MarkerMesh(mesh.clone()));
                mesh
            };
            let root = world
                .spawn((
                    Name::new(cmd.key.clone()),
                    ChildOf(cmd.owner),
                    marker_gid,
                    SkipContentStamp,
                    pointer,
                    lunco_core::SelectableRoot,
                    RuntimeMarkerPart,
                    NativeViewVisibility(true),
                    Visibility::Inherited,
                ))
                .id();
            let visual = world
                .spawn((
                    Name::new("Marker visual"),
                    ChildOf(root),
                    visual_gid,
                    SkipContentStamp,
                    pointer,
                    RuntimeMarkerPart,
                    NativeViewVisibility(true),
                    Visibility::Inherited,
                    Mesh3d(mesh),
                ))
                .id();
            (
                root,
                Part {
                    entity: visual,
                    gid: visual_gid,
                },
            )
        }
    };
    if world.get::<Transform>(root) != Some(&transform) {
        world.entity_mut(root).insert(transform);
    }
    if let Some(cell) = cell {
        if world.get::<CellCoord>(root) != Some(&cell) {
            world.entity_mut(root).insert(cell);
        }
    } else if world.get::<CellCoord>(root).is_some() {
        world.entity_mut(root).remove::<CellCoord>();
    }
    let visual_transform = Transform::from_scale(Vec3::splat(radius));
    if world.get::<Transform>(visual.entity) != Some(&visual_transform) {
        world.entity_mut(visual.entity).insert(visual_transform);
    }
    let look = PbrLook {
        base_color: LinearRgba::new(spec.color[0], spec.color[1], spec.color[2], spec.opacity),
        unlit: true,
        no_shadow_cast: true,
        alpha: SurfaceAlpha::Blend,
        ..default()
    };
    if world.get::<PbrLook>(visual.entity) != Some(&look) {
        world.entity_mut(visual.entity).insert(look);
    }
    let sensor = match sensor_change {
        SensorChange::Keep(part) => part,
        SensorChange::Resize(part, shape) => {
            world.entity_mut(part.entity).insert(shape);
            Some(part)
        }
        SensorChange::Add(gid, shape) => {
            // Avian captures contact event and layer settings on collider insertion.
            let sensor = world
                .spawn((
                    Name::new("Marker sensor"),
                    ChildOf(root),
                    Transform::IDENTITY,
                    gid,
                    SkipContentStamp,
                    lunco_physics::PhysicsOrderKey(format!(
                        "runtime-marker/{}/{}/{sequence}/sensor",
                        parent_gid.get(),
                        cmd.key
                    )),
                    RuntimeMarkerPart,
                    RigidBody::Static,
                    Sensor,
                    CollisionEventsEnabled,
                    CollisionLayers::new(
                        LayerMask(lunco_core::TRIGGER_COLLISION_LAYER),
                        LayerMask::ALL,
                    ),
                ))
                .id();
            world.entity_mut(sensor).insert(shape);
            Some(Part {
                entity: sensor,
                gid,
            })
        }
        SensorChange::Remove(part) => {
            world.despawn(part.entity);
            None
        }
    };
    let marker = RuntimeMarker {
        key: cmd.key,
        actor,
        visual,
        sensor,
        radius: spec.radius,
        color: spec.color,
        opacity: spec.opacity,
        sensor_radius: spec.sensor_radius,
    };
    let data = api_value!({"marker": marker.record(marker_gid,spec.center)});
    world.entity_mut(root).insert(marker);
    Ok(Ack::with_data(OpId::new(), data))
}

#[on_command(SetRuntimeMarker)]
fn on_set_runtime_marker(
    trigger: On<SetRuntimeMarker>,
    active: Res<ActiveCommandId>,
    reply: CommandReply,
    mut commands: Commands,
) -> lunco_core::CommandOutcome {
    let command = trigger.event().clone();
    let origin = active.origin();
    reply.queue(&mut commands, move |world| {
        let actor = lunco_usd_bevy_scene::visibility::presentation_actor(world, origin)?;
        set_marker(world, command, actor)
    })
}
lunco_core::register_commands!(on_set_runtime_marker);

struct RuntimeMarkers;
impl ApiQueryProvider for RuntimeMarkers {
    fn name(&self) -> &'static str {
        "QueryRuntimeMarkers"
    }
    fn schema(&self) -> ApiQuerySchema {
        ApiQuerySchema {name:self.name().into(),description:Some("Read live markers belonging to a mounted scene owner, optionally filtering one key.".into()),
            parameters:Some(vec![
                ApiQueryParameterSchema {name:"owner".into(),type_name:"u64".into(),required:true,description:"Mounted USD owner identity".into(),allowed_values:None},
                ApiQueryParameterSchema {name:"key".into(),type_name:"string".into(),required:false,description:"Limit the result to one marker".into(),allowed_values:None}]),
            exactly_one_of:vec![],response:Some("{ markers: map<key, {marker_id, visual_id, sensor_id, center, radius, color, opacity, sensor_radius}> }".into())}
    }
    fn simulation_read_scope(&self, _params: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::EntityTargets
    }
    fn simulation_entity_reads(&self, params: &ApiValue) -> Vec<GlobalEntityId> {
        api_param_u64(params, "owner")
            .map(GlobalEntityId::from_raw)
            .into_iter()
            .collect()
    }
    fn execute(&self, world: &World, params: &ApiValue) -> lunco_api::ApiQueryResult {
        let fail = |msg: String| lunco_api::ApiQueryError::new(ApiErrorCode::CommandRejected, msg);
        let id = api_param_u64(params, "owner")
            .ok_or_else(|| fail("QueryRuntimeMarkers requires owner".into()))?;
        let owner = world
            .resource::<ApiEntityRegistry>()
            .resolve(&GlobalEntityId::from_raw(id))
            .ok_or_else(|| fail("Marker owner is not live".into()))?;
        validate_owner(world, owner).map_err(fail)?;
        let key = api_param_str(params, "key");
        if key.is_some_and(|key| !valid_key(key)) {
            return Err(fail("Invalid runtime marker key".into()));
        }
        let mut poses = lunco_physics::SimulationPoseReadState::try_new(world)
            .ok_or_else(|| fail("Active physics pose reader unavailable".into()))?;
        let mut entries = Vec::new();
        for (entity, marker) in
            markers(world, owner).filter(|(_, m)| key.is_none_or(|k| k == m.key))
        {
            let gid = world
                .get::<GlobalEntityId>(entity)
                .copied()
                .ok_or_else(|| fail("Marker identity missing".into()))?;
            let center = poses.position(entity).ok_or_else(|| {
                fail(format!(
                    "Marker {} has no current active-frame pose",
                    marker.key
                ))
            })?;
            entries.push((marker.key.clone(), marker.record(gid, center.0.to_array())));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Some(api_value!({"markers": ApiValue::Map(entries)})))
    }
}

fn retire_script_markers(
    event: On<lunco_scripting_bridge_core::ScriptInstanceRetired>,
    mut commands: Commands,
) {
    let actor = event.script;
    commands.queue(move |world: &mut World| {
        let mut query = world.query::<(Entity, &RuntimeMarker)>();
        let owned: Vec<_> = query
            .iter(world)
            .filter(|(_, marker)| marker.actor == Some(actor))
            .map(|(entity, marker)| (entity, marker.clone()))
            .collect();
        for (entity, marker) in owned {
            remove_marker(world, entity, &marker);
        }
    });
}

pub(super) fn install(app: &mut App) {
    app.add_observer(retire_script_markers);
    register_all_commands(app);
    app.register_deferred_command::<SetRuntimeMarker>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(RuntimeMarkers);
}
