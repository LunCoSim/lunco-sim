//! Document-scoped presentation children of mounted USD prims.
use super::*;
use lunco_api::executor::{CommandReply, DeferredCommandAppExt};
use lunco_api::queries::api_param_str;
use lunco_interaction_core::{PointerInteraction, ScenePointerPolicy};
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_bevy_scene::UsdPrimPath;

#[derive(Component)]
pub(super) struct SceneViewObject {
    pub(super) owner: Entity,
    pub(super) key: String,
}

/// A preview retires with either its mounted owner or its source subject.
#[derive(Component)]
#[relationship(relationship_target = SubjectPreviews)]
pub(super) struct PreviewOf(Entity);

#[derive(Component, Default)]
#[relationship_target(relationship = PreviewOf, linked_spawn)]
pub(super) struct SubjectPreviews(Vec<Entity>);

#[derive(Component, Default)]
struct ViewObjects(HashMap<String, Entity>);

#[derive(Clone, Debug, Default, Reflect, serde::Serialize, serde::Deserialize)]
pub enum SceneViewShape {
    #[default]
    Clear,
    Curve {
        anchor: [f64; 3],
        emissive: [f32; 3],
        opacity: f32,
        context_on_secondary: bool,
    },
    MeshPreview {
        subject_id: u64,
        mesh_id: u64,
        color: [f32; 4],
    },
}

/// Set or clear a keyed presentation child of a mounted USD owner.
#[Command]
pub struct SetSceneViewObject {
    /// Mounted source document.
    pub doc_id: u64,
    /// Stable identity of the presentation object's parent.
    #[reflect(@lunco_core::AuthzTarget)]
    pub owner_id: u64,
    /// Stable key within the parent.
    pub key: String,
    /// Requested presentation geometry, or an explicit Clear.
    pub shape: SceneViewShape,
}

fn resolve(world: &World, id: u64) -> Result<Entity, String> {
    world
        .get_resource::<lunco_api::registry::ApiEntityRegistry>()
        .and_then(|registry| registry.resolve(&lunco_core::GlobalEntityId::from_raw(id)))
        .ok_or_else(|| format!("Scene entity {id} is not live"))
}

fn owner(world: &World, doc_id: u64, owner_id: u64) -> Result<Entity, String> {
    let owner = resolve(world, owner_id)?;
    let prim = world
        .get::<UsdPrimPath>(owner)
        .ok_or("Scene view owner must be a USD prim")?;
    let stage = prim.stage_handle.id();
    if !lunco_usd_bevy_scene::is_mounted_scene_stage(world, stage)
        || lunco_usd_bevy_scene::is_preview_only_entity(world, owner)
        || world
            .get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>()
            .and_then(|scenes| lunco_usd_bevy_twin::scene_document_for(scenes, stage))
            != Some(DocumentId::new(doc_id))
    {
        return Err("Scene view owner is outside the requested mounted document".into());
    }
    Ok(owner)
}

fn slot(world: &World, owner: Entity, key: &str) -> Option<Entity> {
    world
        .get::<ViewObjects>(owner)?
        .0
        .get(key)
        .copied()
        .filter(|entity| {
            world
                .get::<SceneViewObject>(*entity)
                .is_some_and(|view| view.owner == owner && view.key == key)
        })
}

#[on_command(SetSceneViewObject)]
fn on_set_scene_view_object(
    trigger: On<SetSceneViewObject>,
    reply: CommandReply,
    mut commands: Commands,
) -> lunco_core::CommandOutcome {
    let command = trigger.event().clone();
    reply.queue(&mut commands, move |world| {
        let parent = owner(world, command.doc_id, command.owner_id)?;
        if command.key.trim().is_empty() {
            return Err("Scene view key must be nonempty".into());
        }
        let existing = slot(world, parent, &command.key);
        if matches!(command.shape, SceneViewShape::Clear) {
            if let Some(entity) = existing {
                world.despawn(entity);
            }
            if let Some(mut slots) = world.get_mut::<ViewObjects>(parent) {
                slots.0.remove(&command.key);
            }
            return Ok(Ack::default());
        }
        let policy = |right| ScenePointerPolicy { left: PointerInteraction::PassThrough, right };
        let (root_transform, subject, mesh, look, child_transform, pointer) = match command.shape {
            SceneViewShape::Curve { anchor, emissive, opacity, context_on_secondary } => {
                let anchor = DVec3::from_array(anchor).as_vec3();
                if !anchor.is_finite() || emissive.iter().any(|c| !c.is_finite() || *c < 0.)
                    || !(0.0..=1.0).contains(&opacity) {
                    return Err("Invalid scene curve anchor or material".into());
                }
                let pointer = policy(if context_on_secondary { PointerInteraction::Context } else { PointerInteraction::PassThrough });
                let look = PbrLook {
                    base_color: LinearRgba::new(0., 0., 0., opacity),
                    emissive: LinearRgba::new(emissive[0], emissive[1], emissive[2], 1.),
                    double_sided: true, no_shadow_cast: true, unshared: true, alpha: SurfaceAlpha::Add,
                    ..default()
                };
                if let Some(entity) = existing {
                    if world.get::<UsdCurveMesh>(entity).is_none() {
                        return Err("Scene view key already denotes a mesh preview".into());
                    }
                    if world.get::<PbrLook>(entity) != Some(&look) {
                        world.entity_mut(entity).insert(look);
                    }
                    if world.get::<ScenePointerPolicy>(entity) != Some(&pointer) {
                        world.entity_mut(entity).insert(pointer);
                    }
                    return Ok(Ack::default());
                }
                let mesh = world.get_resource::<Assets<Mesh>>().ok_or("Mesh assets are unavailable")?.reserve_handle();
                (Transform::from_translation(anchor), None, Mesh3d(mesh), look, None, pointer)
            }
            SceneViewShape::MeshPreview { subject_id, mesh_id, color } => {
                let subject = resolve(world, subject_id)?;
                let source = resolve(world, mesh_id)?;
                if existing.is_some() || world.get::<ChildOf>(subject).map(ChildOf::parent) != Some(parent)
                    || world.get::<ChildOf>(source).map(ChildOf::parent) != Some(subject) {
                    return Err("Mesh preview requires a vacant key and its owner's direct subject and mesh".into());
                }
                if color.iter().any(|c| !c.is_finite() || !(0.0..=1.0).contains(c)) {
                    return Err("Invalid mesh preview color".into());
                }
                let transform = *world.get::<Transform>(subject).ok_or("Mesh preview subject has no transform")?;
                let child = *world.get::<Transform>(source).ok_or("Mesh preview source has no transform")?;
                let mesh = world.get::<Mesh3d>(source).ok_or("Mesh preview source has no geometry")?.clone();
                let look = PbrLook { base_color: LinearRgba::new(color[0],color[1],color[2],color[3]),
                    unlit: true, no_shadow_cast: true, alpha: SurfaceAlpha::Blend, ..default() };
                (transform, Some(subject), mesh, look, Some(child), policy(PointerInteraction::PassThrough))
            }
            SceneViewShape::Clear => unreachable!(),
        };
        let entity = world.spawn((ChildOf(parent), root_transform,
            lunco_core::Provenance::Derived { parent: command.owner_id, role: format!("scene-view/{}", command.key) },
            SceneViewObject { owner: parent, key: command.key.clone() },
        )).id();
        if let Some((child, subject)) = child_transform.zip(subject) {
            world.entity_mut(entity).insert(PreviewOf(subject));
            if let Some(cell) = world.get::<CellCoord>(subject).copied() {
                world.entity_mut(entity).insert(cell);
            }
            world.entity_mut(entity).insert((Visibility::Inherited, Pickable::IGNORE, pointer));
            world.spawn((ChildOf(entity), child, mesh, look, Visibility::Inherited, Pickable::IGNORE));
        } else {
            world.entity_mut(entity).insert((UsdCurveMesh, TransientUsdCurveView, mesh, look, Visibility::Hidden,
                Pickable::default(), pointer));
        }
        world.entity_mut(parent).entry::<ViewObjects>().or_default().into_mut().0.insert(command.key, entity);
        Ok(Ack::default())
    })
}

struct InspectSceneViewObject;

impl ApiQueryProvider for InspectSceneViewObject {
    fn name(&self) -> &'static str {
        "InspectSceneViewObject"
    }
    fn schema(&self) -> ApiQuerySchema {
        ApiQuerySchema {
            name: self.name().into(),
            description: Some(
                "Resolve a presentation child and its optional preview subject.".into(),
            ),
            parameters: Some(
                [
                    ("doc_id", "u64", "Mounted document"),
                    ("owner_id", "u64", "Stable owner identity"),
                    ("key", "String", "Presentation key"),
                ]
                .map(|(name, ty, description)| ApiQueryParameterSchema {
                    name: name.into(),
                    type_name: ty.into(),
                    required: true,
                    description: description.into(),
                    allowed_values: None,
                })
                .to_vec(),
            ),
            exactly_one_of: vec![],
            response: Some("{ exists, entity_id, subject_id, subject_path, left, right }".into()),
        }
    }
    fn simulation_read_scope(&self, _: &ApiValue) -> SimulationQueryReadScope {
        SimulationQueryReadScope::EntityTargets
    }
    fn simulation_entity_reads(&self, params: &ApiValue) -> Vec<lunco_core::GlobalEntityId> {
        api_param_u64(params, "owner_id")
            .map(lunco_core::GlobalEntityId::from_raw)
            .into_iter()
            .collect()
    }
    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let invalid =
            |msg: &str| ApiQueryError::new(ApiErrorCode::DeserializationError, msg.to_owned());
        let doc = api_param_u64(params, "doc_id").ok_or_else(|| invalid("Expected doc_id"))?;
        let id = api_param_u64(params, "owner_id").ok_or_else(|| invalid("Expected owner_id"))?;
        let key = api_param_str(params, "key").ok_or_else(|| invalid("Expected key"))?;
        let owner = owner(world, doc, id).map_err(|msg| invalid(&msg))?;
        let entity = slot(world, owner, key);
        let admitted_id = |entity: Entity| {
            let gid = world.get::<lunco_core::GlobalEntityId>(entity)?;
            (resolve(world, gid.get()).ok() == Some(entity)).then_some(gid.get())
        };
        let subject = entity
            .and_then(|entity| world.get::<PreviewOf>(entity))
            .map(|subject| subject.0);
        let subject_path = subject.and_then(|entity| world.get::<UsdPrimPath>(entity));
        let pointer = entity.and_then(|e| world.get::<ScenePointerPolicy>(e));
        Ok(Some(
            api_value!({"exists":entity.is_some(),"entity_id":entity.and_then(admitted_id),
            "subject_id":subject.and_then(admitted_id), "subject_path":subject_path.map(|prim|prim.path.clone()),
            "left":pointer.map(|p|pointer_interaction_name(p.left)), "right":pointer.map(|p|pointer_interaction_name(p.right))}),
        ))
    }
}

pub(super) fn install(app: &mut App) {
    app.register_type::<SceneViewShape>();
    __register_on_set_scene_view_object(app);
    app.register_deferred_command::<SetSceneViewObject>();
    app.world_mut()
        .resource_mut::<lunco_api::queries::ApiQueryRegistry>()
        .register(InspectSceneViewObject);
}
