//! Scene-owned generated cuboids. Preparation reads composed material/group
//! definitions once. Admission and retirement change native entities only.
use avian3d::prelude::*;
use bevy::{
    ecs::system::SystemState,
    math::{DQuat, DVec3},
    prelude::*,
};
use big_space::prelude::{CellCoord, Grid};
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry, api_param_str, api_param_u64};
use lunco_api::registry::ApiEntityRegistry;
use lunco_api_core::{ApiErrorCode, ApiValue, api_value};
use lunco_command_contracts::{Ack, OpId};
use lunco_core::{
    ActiveCommandId, AppSceneExt, GlobalEntityId, SceneMountState, SceneResource, on_command,
};
use lunco_doc::DocumentId;
use lunco_hooks::HookValue;
use lunco_scene_command_contracts::{ReplaceRuntimeObjects, RuntimeObjectSpec};
use lunco_usd_avian::material::PhysicsMaterial;
use lunco_usd_bevy_scene::{NativeViewVisibility, UsdPrimPath};
use lunco_usd_bevy_stage::{UsdStageAsset, canonical::CanonicalStages};
use lunco_usd_bevy_twin::DocBackedTwinScenes;
use std::collections::{BTreeMap, HashSet};

#[derive(Component)]
struct RuntimeObject {
    group: String,
    key: String,
    size: [f64; 3],
}

#[derive(Resource, Default)]
struct RuntimeObjectSequence(u64);
impl SceneResource for RuntimeObjectSequence {}

pub(super) fn next_sequence(world: &World) -> Result<u64, String> {
    world
        .resource::<RuntimeObjectSequence>()
        .0
        .checked_add(1)
        .ok_or_else(|| "Runtime object identity sequence exhausted".into())
}

pub(super) fn commit_sequence(world: &mut World, sequence: u64) {
    world.resource_mut::<RuntimeObjectSequence>().0 = sequence;
}

pub(super) fn derived_gid(
    parent: u64,
    role: String,
    registry: &ApiEntityRegistry,
) -> Result<GlobalEntityId, String> {
    let provenance = lunco_core::identity::Provenance::Derived { parent, role };
    let gid = GlobalEntityId::from_raw(
        lunco_core::identity::derive_id(&provenance)
            .ok_or("Cannot derive runtime object identity")?,
    );
    if gid.get() == 0 || registry.resolve(&gid).is_some() {
        return Err(format!("Runtime identity collision for {provenance:?}"));
    }
    Ok(gid)
}

struct PreparedObject {
    spec: RuntimeObjectSpec,
    cell: CellCoord,
    local: Transform,
    collider: Option<Collider>,
    mass: Option<(Mass, AngularInertia, CenterOfMass)>,
    material: Option<PhysicsMaterial>,
    layers: Option<CollisionLayers>,
    gid: GlobalEntityId,
}

fn root_for_document(world: &World, doc: DocumentId) -> Result<Entity, String> {
    let roots = world
        .get_resource::<SceneMountState>()
        .ok_or("Scene mount state unavailable")?;
    let backed = world
        .get_resource::<DocBackedTwinScenes>()
        .ok_or("Scene document ownership unavailable")?;
    let mut matching = roots.roots().filter(|root| {
        world.get::<UsdPrimPath>(*root).is_some_and(|prim| {
            lunco_usd_bevy_twin::scene_document_for(backed, prim.stage_handle.id()) == Some(doc)
        })
    });
    let root = matching
        .next()
        .ok_or("Document has no current mounted scene")?;
    if matching.next().is_some() {
        return Err("Document has ambiguous mounted roots".into());
    }
    Ok(root)
}

fn group_members(
    world: &World,
    root: Entity,
    group: &str,
) -> BTreeMap<String, (Entity, GlobalEntityId, [f64; 3])> {
    let mut members = BTreeMap::new();
    if let Some(children) = world.get::<Children>(root) {
        for &entity in children {
            if let Some(object) = world.get::<RuntimeObject>(entity)
                && object.group == group
                && let Some(id) = world.get::<GlobalEntityId>(entity)
            {
                members.insert(object.key.clone(), (entity, *id, object.size));
            }
        }
    }
    members
}

pub(super) fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn prepare(
    world: &mut World,
    root: Entity,
    command: &ReplaceRuntimeObjects,
    sequence: u64,
) -> Result<Vec<PreparedObject>, String> {
    if command.objects.is_empty() {
        return Ok(Vec::new());
    }
    let frame = world
        .get_resource::<lunco_spatial::ActivePhysicsFrame>()
        .ok_or("Active physics frame unavailable")?
        .0;
    let root_gid = world
        .get::<GlobalEntityId>(root)
        .ok_or("Mounted root has no admitted identity")?
        .get();
    let stage = world
        .get::<UsdPrimPath>(root)
        .ok_or("Mounted root has no USD source")?
        .stage_handle
        .id();
    let doc = DocumentId::new(command.doc_id);
    let generation = world.get_resource::<lunco_doc_bevy::DocumentRegistry<lunco_usd_document::document::UsdDocument>>()
        .and_then(|registry| registry.host(doc)).ok_or("Runtime object source document is closed")?.generation();
    if world
        .resource::<DocBackedTwinScenes>()
        .applied_stage_generation(doc, stage)
        != Some(generation)
    {
        return Err(
            "Runtime object preparation requires the current mounted document revision".into(),
        );
    }
    let mut spatial: SystemState<(
        Query<&ChildOf>,
        Query<&Grid>,
        Query<(Option<&CellCoord>, &Transform)>,
    )> = SystemState::new(world);
    let (parents, grids, poses) = spatial.get(world).map_err(|error| error.to_string())?;
    if grids.get(root).is_err() || grids.get(frame).is_err() {
        return Err("Runtime objects require a mounted physics grid".into());
    }
    if world.get_resource::<Assets<Mesh>>().is_none() {
        return Err("Native mesh assets unavailable".into());
    }
    let stages = world
        .get_non_send::<CanonicalStages>()
        .ok_or("Canonical stage registry unavailable")?;
    let asset = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|assets| assets.get(stage))
        .ok_or("Mounted stage unavailable")?;
    let (reader, _) = stages.reader_for_composed_query(
        stage,
        asset,
        lunco_usd_bevy_stage::canonical::ComposedReadRequirement::Native,
    );
    let groups = world
        .get_resource::<lunco_usd_avian_filters::collision_groups::CollisionGroupTables>()
        .and_then(|tables| tables.get(stage));
    let ids = world.resource::<ApiEntityRegistry>();
    let mut keys = HashSet::new();
    let mut identities = HashSet::new();
    let mut result = Vec::with_capacity(command.objects.len());
    for spec in &command.objects {
        if !valid_key(&spec.key) || !keys.insert(&spec.key) {
            return Err(format!(
                "Invalid or repeated runtime object key {:?}",
                spec.key
            ));
        }
        let size = DVec3::from_array(spec.size);
        let render_size = size.as_vec3();
        if !size.is_finite()
            || !render_size.is_finite()
            || render_size.min_element() <= 0.0
            || spec.color.iter().any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(format!("Invalid dimensions or color for {}", spec.key));
        }
        if (!spec.collider
            && (spec.mass.is_some()
                || spec.physics_material.is_some()
                || spec.collision_group.is_some()))
            || (!spec.active && spec.mass.is_none())
        {
            return Err(format!("Invalid physics participation for {}", spec.key));
        }
        let translation = DVec3::from_array(spec.translation);
        if !translation.is_finite() {
            return Err(format!("Invalid position for {}", spec.key));
        }
        let (Some(cell), local_position, rotation) =
            lunco_spatial::coords::pose_in_grid_to_parent_storage(
                translation,
                DQuat::IDENTITY,
                root,
                frame,
                &parents,
                &grids,
                &poses,
            )
            .ok_or("Runtime object is outside the active physics frame")?
        else {
            return Err("Runtime object parent is not a grid".into());
        };
        if !local_position.is_finite() || !rotation.is_finite() {
            return Err("Runtime object pose exceeds storage range".into());
        }
        let collider = if spec.collider {
            let collider = Collider::cuboid(render_size.x, render_size.y, render_size.z);
            if !lunco_physics::avian_backend_collider_shape_is_valid(&collider) {
                return Err(format!("Invalid native collider for {}", spec.key));
            }
            Some(collider)
        } else {
            None
        };
        let mass = spec
            .mass
            .map(|mass| {
                let extent_squared = size * size;
                let principal = DVec3::new(
                    extent_squared.y + extent_squared.z,
                    extent_squared.x + extent_squared.z,
                    extent_squared.x + extent_squared.y,
                ) * (mass / 12.0);
                let facts = lunco_physics::body_mass::checked_mass_properties(
                    DVec3::ZERO,
                    mass,
                    principal,
                    DQuat::IDENTITY,
                )?;
                lunco_physics::body_mass::native_mass_properties(&facts)
            })
            .transpose()
            .map_err(|error| format!("{}: {error}", spec.key))?;
        let material = spec
            .physics_material
            .as_ref()
            .map(|path| {
                let path =
                    openusd::sdf::Path::new(path.as_str()).map_err(|error| error.to_string())?;
                lunco_usd_avian::material::read_material_prim(&reader, &path)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("{path} is not a composed physics material"))
            })
            .transpose()?;
        let layers = spec
            .collision_group
            .as_ref()
            .map(|path| {
                groups
                    .and_then(|table| table.layers_for_group(path))
                    .ok_or_else(|| format!("Collision group {path} is not admitted"))
            })
            .transpose()?;
        let gid = derived_gid(
            root_gid,
            format!("runtime-object/{}/{sequence}/{}", command.group, spec.key),
            ids,
        )?;
        if !identities.insert(gid) {
            return Err(format!("Runtime identity collision for {}", spec.key));
        }
        result.push(PreparedObject {
            spec: spec.clone(),
            cell,
            local: Transform::from_translation(local_position).with_rotation(rotation.as_quat()),
            collider,
            mass,
            material,
            layers,
            gid,
        });
    }
    Ok(result)
}

fn replace(world: &mut World, command: ReplaceRuntimeObjects) -> Result<Ack, String> {
    if !valid_key(&command.group) || command.objects.len() > 4096 {
        return Err("Invalid runtime object group or object count".into());
    }
    if world
        .resource::<lunco_core_session::NetworkRole>()
        .is_networked()
    {
        return Err(
            "Generated runtime object groups currently require a standalone session".into(),
        );
    }
    let root = root_for_document(world, DocumentId::new(command.doc_id))?;
    let sequence = next_sequence(world)?;
    let prepared = prepare(world, root, &command, sequence)?;
    let existing = group_members(world, root, &command.group);
    let affects_physics = prepared.iter().any(|object| object.collider.is_some())
        || existing
            .values()
            .any(|(entity, _, _)| world.get::<Collider>(*entity).is_some());
    if affects_physics
        && world
            .get_resource::<lunco_time::TimeTransport>()
            .is_none_or(|transport| transport.mode != lunco_time::TransportMode::Paused)
    {
        return Err("Pause simulation before replacing a physical runtime object group".into());
    }
    // Everything that can reject the plan precedes its first scene mutation.
    commit_sequence(world, sequence);
    if let Some(mut selected) = world.get_resource_mut::<lunco_scene_selection::SelectedEntities>()
    {
        selected
            .entities
            .retain(|entity| !existing.values().any(|(old, _, _)| old == entity));
    }
    for (entity, _, _) in existing.values() {
        world.despawn(*entity);
    }
    let mut entities = Vec::with_capacity(prepared.len());
    for object in prepared {
        let PreparedObject {
            spec,
            cell,
            local,
            collider,
            mass,
            material,
            layers,
            gid,
        } = object;
        let size = Vec3::from_array(spec.size.map(|v| v as f32));
        let mesh = world
            .resource_mut::<Assets<Mesh>>()
            .add(Cuboid::from_size(size));
        let entity = world
            .spawn((
                Name::new(spec.key.clone()),
                RuntimeObject {
                    group: command.group.clone(),
                    key: spec.key.clone(),
                    size: spec.size,
                },
                ChildOf(root),
                cell,
                local,
                gid,
                lunco_core_session::SkipContentStamp,
                lunco_core::SelectableRoot,
                NativeViewVisibility(spec.visible),
                if spec.visible {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                },
                Mesh3d(mesh),
                lunco_render::PbrLook {
                    base_color: LinearRgba::rgb(spec.color[0], spec.color[1], spec.color[2]),
                    ..default()
                },
            ))
            .id();
        if let Some((mass, inertia, center)) = mass {
            world.entity_mut(entity).insert((
                mass,
                inertia,
                center,
                NoAutoMass,
                NoAutoAngularInertia,
                NoAutoCenterOfMass,
            ));
        }
        if let Some(layers) = layers {
            world.entity_mut(entity).insert(layers);
        }
        if let Some(material) = material {
            if let Some(friction) = material.friction() {
                world.entity_mut(entity).insert(friction);
                if (friction.static_coefficient - friction.dynamic_coefficient).abs() > f32::EPSILON
                {
                    world
                        .entity_mut(entity)
                        .insert(ActiveCollisionHooks::MODIFY_CONTACTS);
                }
            }
            if let Some(restitution) = material.restitution() {
                world.entity_mut(entity).insert(restitution);
            }
        }
        if let Some(collider) = collider {
            world.entity_mut(entity).insert((
                lunco_physics::PhysicsOrderKey(format!(
                    "runtime/{}/{sequence}/{}",
                    command.group, spec.key
                )),
                collider,
            ));
            if spec.mass.is_some() {
                world.entity_mut(entity).insert((
                    RigidBody::Dynamic,
                    lunco_core::Mobility::Dynamic,
                    lunco_core::PhysicsStateReady,
                ));
                if !spec.active {
                    lunco_physics::participation::initialize_inactive_body(world, entity);
                }
            } else {
                world.entity_mut(entity).insert(RigidBody::Static);
            }
        }
        entities.push((spec.key, HookValue::UInt(gid.get())));
    }
    Ok(Ack::with_data(
        OpId::new(),
        HookValue::map([("entities", HookValue::Map(entities))]),
    ))
}

#[on_command(ReplaceRuntimeObjects)]
fn on_replace_runtime_objects(
    trigger: On<ReplaceRuntimeObjects>,
    active: Res<ActiveCommandId>,
    reply: lunco_api::executor::CommandReply,
    mut commands: Commands,
) -> lunco_core::CommandOutcome {
    let spec = trigger.event().clone();
    let origin = active.origin();
    reply.queue(&mut commands, move |world| {
        lunco_usd_bevy_scene::visibility::presentation_actor(world, origin)?;
        replace(world, spec)
    })
}
lunco_core::register_commands!(on_replace_runtime_objects);

struct RuntimeObjectsQuery;
impl ApiQueryProvider for RuntimeObjectsQuery {
    fn name(&self) -> &'static str {
        "QueryRuntimeObjects"
    }
    fn schema(&self) -> lunco_api_core::ApiQuerySchema {
        use lunco_api_core::ApiQueryParameterSchema;
        lunco_api_core::ApiQuerySchema {
            name: self.name().into(),
            description: Some("Read the current live objects and full dimensions of a mounted document's named runtime group.".into()),
            parameters: Some(vec![
                ApiQueryParameterSchema { name: "doc_id".into(), type_name: "u64".into(), required: true, description: "Owning mounted USD document identity".into(), allowed_values: None },
                ApiQueryParameterSchema { name: "group".into(), type_name: "string".into(), required: true, description: "Runtime object group key".into(), allowed_values: None },
            ]),
            exactly_one_of: vec![],
            response: Some("{ entities: map<string, u64>, sizes: map<string, [f64; 3]> }".into()),
        }
    }
    fn execute(&self, world: &World, params: &ApiValue) -> lunco_api::ApiQueryResult {
        let doc = api_param_u64(params, "doc_id").ok_or_else(|| {
            lunco_api::ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "QueryRuntimeObjects requires doc_id",
            )
        })?;
        let group = api_param_str(params, "group")
            .filter(|name| valid_key(name))
            .ok_or_else(|| {
                lunco_api::ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "QueryRuntimeObjects requires a valid group",
                )
            })?;
        let root = root_for_document(world, DocumentId::new(doc))
            .map_err(|error| lunco_api::ApiQueryError::new(ApiErrorCode::CommandRejected, error))?;
        let members = group_members(world, root, group);
        let entities = ApiValue::map(
            members
                .iter()
                .map(|(key, (_, id, _))| (key.clone(), ApiValue::UInt(id.get()))),
        );
        let sizes = ApiValue::map(
            members
                .iter()
                .map(|(key, (_, _, size))| (key.clone(), api_value!([size[0], size[1], size[2]]))),
        );
        Ok(Some(api_value!({"entities": entities, "sizes": sizes})))
    }
}

pub(crate) fn install(app: &mut App) {
    use lunco_api::executor::DeferredCommandAppExt;
    app.init_scene_resource::<RuntimeObjectSequence>();
    register_all_commands(app);
    app.register_deferred_command::<ReplaceRuntimeObjects>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(RuntimeObjectsQuery);
}
