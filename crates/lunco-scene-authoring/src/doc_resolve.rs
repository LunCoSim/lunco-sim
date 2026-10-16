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
    let declared_type = world
        .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
        .and_then(|stages| stages.get(prim.stage_handle.id()))
        .and_then(|stage| stage.view().attr_type_name(&shader_sdf, &attribute_name));
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
/// Resolves the native universal surface producer through UsdShade NodeGraphs
/// on the live canonical stage (building it from the asset's recipe if it has
/// not been built yet). Shared, because the two places that edit a look — the
/// Inspector panel and the `SetObjectProperty` command — must agree on WHERE the look
/// lives, or one of them will scribble `inputs:*` somewhere no other DCC reads it
/// back from.
pub fn bound_shader_prim(world: &mut World, prim: &UsdPrimPath) -> Result<Option<String>, String> {
    let id = prim.stage_handle.id();
    let mesh_sdf = SdfPath::new(&prim.path).map_err(|error| error.to_string())?;

    let recipe = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|stages| stages.get(&prim.stage_handle))
        .and_then(|a| a.recipe.clone());
    if let Some(mut canonical) =
        world.get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
    {
        if canonical.get(id).is_none() {
            if let Some(r) = recipe.as_ref() {
                canonical.get_or_build(id, r);
            }
        }
    }
    let canonical = world
        .get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
        .ok_or_else(|| "native USD stages are unavailable for material authoring".to_string())?;
    let view = canonical
        .get(id)
        .ok_or_else(|| format!("{} has no native stage for material authoring", prim.path))?
        .view();
    view.bound_shader(&mesh_sdf)
        .map(|path| path.map(|path| path.to_string()))
}

/// The composed `apiSchemas` applied to `prim`, as owned strings — what
/// `ensure_preview_surface_ops` needs so the `MaterialBindingAPI` it applies
/// composes WITH the prim's existing applied schemas instead of erasing them.
/// Empty when the prim applies none (or the stage is not built yet).
pub fn geom_api_schemas(world: &mut World, prim: &UsdPrimPath) -> Vec<String> {
    let id = prim.stage_handle.id();
    let Ok(sdf) = SdfPath::new(&prim.path) else {
        return Vec::new();
    };
    let recipe = world
        .get_resource::<Assets<UsdStageAsset>>()
        .and_then(|stages| stages.get(&prim.stage_handle))
        .and_then(|a| a.recipe.clone());
    if let Some(mut canonical) =
        world.get_non_send_mut::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
    {
        if canonical.get(id).is_none() {
            if let Some(r) = recipe.as_ref() {
                canonical.get_or_build(id, r);
            }
        }
    }
    let Some(canonical) = world.get_non_send::<lunco_usd_bevy_stage::canonical::CanonicalStages>()
    else {
        return Vec::new();
    };
    let Some(view) = canonical.get(id).map(|c| c.view()) else {
        return Vec::new();
    };
    view.stage()
        .prim(sdf)
        .expect("validated USD path")
        .api_schemas()
        .map(|v| v.iter().map(|s| s.as_str().to_string()).collect())
        .unwrap_or_default()
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
    let prim = q_prim.get(entity).ok()?;
    let doc = resolve_doc_for_stage(prim.stage_handle.id(), backed, usd_registry)?;
    let host = usd_registry.host(doc)?;
    let prim_sdf = SdfPath::new(&prim.path).ok()?;
    let stage_id = prim.stage_handle.id();
    let (reader, _) = stages?.reader_for_composed_query(stage_id, assets?.get(stage_id)?);
    if !reader.has_prim(&prim_sdf) || !reader.is_active(&prim_sdf).ok()? {
        return None;
    }
    let owned = host.document().data().spec(&prim_sdf).is_some()
        || host.document().runtime_data().spec(&prim_sdf).is_some();
    let composed = reader.has_composition_arc(&prim_sdf).ok()?;
    (owned || composed).then(|| (doc, prim.path.clone()))
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
    entity: Entity,
    q_prim: &Query<&UsdPrimPath>,
    usd_registry: &DocumentRegistry<UsdDocument>,
    doc: lunco_doc::DocumentId,
    stages: Option<&lunco_usd_bevy_stage::canonical::CanonicalStages>,
    assets: Option<&Assets<UsdStageAsset>>,
) -> Option<(lunco_doc::DocumentId, String, DeleteTarget)> {
    let host = usd_registry.host(doc)?;
    let prim = q_prim.get(entity).ok()?;
    let prim_sdf = SdfPath::new(&prim.path).ok()?;
    let root_authored = host.document().data().spec(&prim_sdf).is_some();
    let runtime_authored = host.document().runtime_data().spec(&prim_sdf).is_some();
    let stage_id = prim.stage_handle.id();
    let (reader, _) = stages?.reader_for_composed_query(stage_id, assets?.get(stage_id)?);
    if !reader.has_prim(&prim_sdf) {
        return None;
    }
    let under_composed_arc = reader.has_composition_arc(&prim_sdf).ok()?;

    let target = if runtime_authored && !root_authored && !under_composed_arc {
        DeleteTarget::RemoveRuntime
    } else if root_authored || runtime_authored || under_composed_arc {
        DeleteTarget::DeactivateRuntime
    } else {
        return None;
    };
    Some((doc, prim.path.clone(), target))
}
