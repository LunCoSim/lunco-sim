//! Which document backs this entity, and where does its look live — the two
//! questions every authoring path has to answer before it can write a USD op.
//!
//! Document and native surface-terminal ownership is shared by command and
//! Inspector authoring paths.

use bevy::asset::AssetId;
use bevy::prelude::*;
use lunco_doc_bevy::DocumentRegistry;
use lunco_materials::ParamValue;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_bevy_stage::{UsdRead, UsdStageAsset};
use lunco_usd_document::document::UsdDocument;
use openusd::sdf::Path as SdfPath;

/// The exact USD destination and literal for one dynamic shader parameter.
///
/// The destination is the bound `UsdShade.Shader` input, not a geometry
/// `primvars:` guess. The declaration comes from the composed stage when the
/// input already exists, preserving USD roles and array shape; a new input uses
/// the canonical scalar/vector type for the reflected shader value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShaderParameterUsdTarget {
    pub shader_path: String,
    pub attribute_name: String,
    pub type_name: String,
    pub literal: String,
}

/// Resolve one typed shader parameter to its standard USD `Shader.inputs:*`
/// destination. This is shared by the Inspector and the generic
/// `SetObjectProperty` persistence observer so they cannot author different
/// representations of the same look.
pub fn resolve_shader_parameter_usd_target(
    world: &mut World,
    prim: &UsdPrimPath,
    name: &str,
    value: &ParamValue,
) -> Result<ShaderParameterUsdTarget, String> {
    let shader_path = bound_shader_prim(world, prim)?.ok_or_else(|| {
        format!(
            "{} has no bound USD Shader prim; shader parameter cannot be authored",
            prim.path
        )
    })?;
    let shader_sdf = SdfPath::new(&shader_path)
        .map_err(|error| format!("bound Shader path `{shader_path}` is invalid: {error}"))?;
    let attribute_name = if name.starts_with("inputs:") {
        name.to_owned()
    } else {
        format!("inputs:{name}")
    };
    let declared_type = composed_reader(world, prim)?.attr_type_name(&shader_sdf, &attribute_name);
    let type_name =
        declared_type.unwrap_or_else(|| canonical_shader_parameter_type(value).to_owned());
    let literal = canonical_shader_parameter_literal(&type_name, value)?;
    Ok(ShaderParameterUsdTarget {
        shader_path,
        attribute_name,
        type_name,
        literal,
    })
}

fn canonical_shader_parameter_type(value: &ParamValue) -> &'static str {
    match value {
        ParamValue::F32(_) => "float",
        ParamValue::I32(_) => "int",
        ParamValue::U32(_) => "uint",
        ParamValue::Vec2(_) => "float2",
        ParamValue::Vec3(_) => "float3",
        ParamValue::Vec4(_) => "float4",
    }
}

fn canonical_shader_parameter_literal(
    type_name: &str,
    value: &ParamValue,
) -> Result<String, String> {
    let candidates = match value {
        ParamValue::F32(value) => vec![(value.to_string(), 1)],
        ParamValue::I32(value) => vec![(value.to_string(), 1)],
        ParamValue::U32(value) => vec![(value.to_string(), 1)],
        ParamValue::Vec2(value) => vec![(format!("({}, {})", value[0], value[1]), 2)],
        ParamValue::Vec3(value) => vec![(format!("({}, {}, {})", value[0], value[1], value[2]), 3)],
        ParamValue::Vec4(value) => vec![
            (
                format!("({}, {}, {}, {})", value[0], value[1], value[2], value[3]),
                4,
            ),
            (format!("({}, {}, {})", value[0], value[1], value[2]), 3),
        ],
    };
    let declared_components = usd_numeric_component_count(type_name);
    for (candidate, components) in candidates {
        if declared_components.is_some_and(|declared| declared != components) {
            continue;
        }
        let literal = if type_name.ends_with("[]") {
            format!("[{candidate}]")
        } else {
            candidate
        };
        if lunco_usd_authoring::author::parse_attribute_value(type_name, &literal).is_ok() {
            return Ok(literal);
        }
    }
    Err(format!("{type_name} does not accept this parameter shape"))
}

fn usd_numeric_component_count(type_name: &str) -> Option<usize> {
    let base = type_name.strip_suffix("[]").unwrap_or(type_name);
    match base {
        "float2" | "double2" | "half2" => Some(2),
        "float3" | "double3" | "half3" | "color3f" | "color3d" | "color3h" | "point3f"
        | "point3d" | "point3h" | "vector3f" | "vector3d" | "vector3h" | "normal3f"
        | "normal3d" | "normal3h" => Some(3),
        "float4" | "double4" | "half4" | "color4f" | "color4d" | "color4h" | "quatf" | "quatd"
        | "quath" => Some(4),
        "float" | "double" | "half" | "int" | "int64" | "uint" | "uint64" | "bool" => Some(1),
        _ => None,
    }
}

/// The `UsdPreviewSurface` Shader prim bound to `prim`'s geometry, or `None` when it
/// has no material yet.
///
/// Resolves the native universal surface producer through composed UsdShade NodeGraphs.
pub fn bound_shader_prim(world: &World, prim: &UsdPrimPath) -> Result<Option<String>, String> {
    composed_reader(world, prim)?
        .bound_shader(&SdfPath::new(&prim.path).map_err(|e| e.to_string())?)
        .map(|shader| shader.map(|path| path.to_string()))
}

/// The applied schemas preserved when adding a material binding.
pub fn geom_api_schemas(world: &World, prim: &UsdPrimPath) -> Result<Vec<String>, String> {
    let path = SdfPath::new(&prim.path).map_err(|e| e.to_string())?;
    Ok(composed_reader(world, prim)?.api_schemas(&path))
}

pub fn composed_reader<'w>(
    world: &'w World,
    prim: &UsdPrimPath,
) -> Result<lunco_usd_bevy_stage::UsdReadSource<'w>, String> {
    let id = prim.stage_handle.id();
    let stages = world
        .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
        .ok_or("USD projection registry unavailable")?;
    let asset = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|assets| assets.get(id))
        .ok_or_else(|| format!("stage unavailable for {}", prim.path))?;
    Ok(stages
        .reader_for_composed_query(
            id,
            asset,
            lunco_usd_bevy_stage::canonical::ComposedReadRequirement::Native,
        )
        .0)
}

/// Resolve the document holding the entity's exact projected stage lease.
pub fn resolve_doc_for_entity(world: &World, entity: Entity) -> Option<lunco_doc::DocumentId> {
    let prim = world.get::<UsdPrimPath>(entity)?;
    resolve_doc_for_stage(
        prim.stage_handle.id(),
        world.get_resource::<lunco_usd_bevy_twin::DocBackedTwinScenes>(),
        world.get_resource::<DocumentRegistry<UsdDocument>>()?,
    )
}

/// Resolve only through the existing document projection owner.
pub fn resolve_doc_for_stage(
    stage_id: AssetId<UsdStageAsset>,
    backed: Option<&lunco_usd_bevy_twin::DocBackedTwinScenes>,
    registry: &DocumentRegistry<UsdDocument>,
) -> Option<lunco_doc::DocumentId> {
    lunco_usd_bevy_twin::scene_document_for(backed?, stage_id).filter(|doc| registry.contains(*doc))
}

/// Resolve a live prim against its owning document and canonical composition.
/// Transient view-only prims have no persistent authored destination.
pub fn authorable_prim(
    entity: Entity,
    q_prim: &Query<&UsdPrimPath>,
    usd_registry: &DocumentRegistry<UsdDocument>,
    backed: Option<&lunco_usd_bevy_twin::DocBackedTwinScenes>,
    stages: Option<&lunco_usd_bevy_stage::canonical::CanonicalStages>,
    assets: Option<&Assets<UsdStageAsset>>,
) -> Option<(lunco_doc::DocumentId, String)> {
    authorable_prim_path(
        q_prim.get(entity).ok()?,
        usd_registry,
        backed,
        stages,
        assets,
    )
}

pub(crate) fn authorable_entity(
    world: &World,
    entity: Entity,
) -> Option<(lunco_doc::DocumentId, String)> {
    authorable_prim_path(
        world.get::<UsdPrimPath>(entity)?,
        world.get_resource()?,
        world.get_resource(),
        world.get_non_send(),
        world.get_resource(),
    )
}

fn authorable_prim_path(
    prim: &UsdPrimPath,
    usd_registry: &DocumentRegistry<UsdDocument>,
    backed: Option<&lunco_usd_bevy_twin::DocBackedTwinScenes>,
    stages: Option<&lunco_usd_bevy_stage::canonical::CanonicalStages>,
    assets: Option<&Assets<UsdStageAsset>>,
) -> Option<(lunco_doc::DocumentId, String)> {
    delete_target(prim, usd_registry, backed, stages, assets).map(|(doc, path, _)| (doc, path))
}

/// The generic persistent-delete target for a document-backed prim.
///
/// Runtime-only prims can be removed from the runtime layer. A base-authored
/// prim, or a child supplied by a reference/payload, must instead receive a
/// stronger runtime `active = false` opinion: `RemovePrim @runtime@` cannot
/// remove a spec that the runtime layer does not own. This is the standard USD
/// non-destructive override and keeps the base Twin layer unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteTarget {
    RemoveRuntime,
    DeactivateRuntime,
}

/// Resolve the document/path and the USD operation shape for a persistent
/// generic delete.
pub fn delete_target(
    prim: &UsdPrimPath,
    usd_registry: &DocumentRegistry<UsdDocument>,
    backed: Option<&lunco_usd_bevy_twin::DocBackedTwinScenes>,
    stages: Option<&lunco_usd_bevy_stage::canonical::CanonicalStages>,
    assets: Option<&Assets<UsdStageAsset>>,
) -> Option<(lunco_doc::DocumentId, String, DeleteTarget)> {
    let stage_id = prim.stage_handle.id();
    let backed = backed?;
    let doc = resolve_doc_for_stage(stage_id, Some(backed), usd_registry)?;
    let host = usd_registry.host(doc)?;
    if backed.applied_stage_generation(doc, stage_id) != Some(host.generation()) {
        return None;
    }
    let path = SdfPath::new(&prim.path).ok()?;
    let (reader, _) = stages?.reader_for_composed_query(
        stage_id,
        assets?.get(stage_id)?,
        lunco_usd_bevy_stage::canonical::ComposedReadRequirement::Native,
    );
    if !reader.has_prim(&path) || !reader.is_active(&path).ok()? {
        return None;
    }
    let target = if reader.has_source_spec(&path).ok()? {
        DeleteTarget::DeactivateRuntime
    } else if host.document().runtime_data().spec(&path).is_some() {
        DeleteTarget::RemoveRuntime
    } else {
        return None;
    };
    Some((doc, prim.path.clone(), target))
}
