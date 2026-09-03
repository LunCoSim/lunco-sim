//! Which document backs this entity, and where does its look live — the two
//! questions every authoring path has to answer before it can write a USD op.
//!
//! Both helpers used to sit in `ui::inspector` because a panel happened to be the
//! first caller. That made them `ui`-only, while `commands.rs` — declared
//! headless-safe, and the module a `--no-ui` server depends on for
//! `SpawnCommandPlugin` — reached into `crate::ui::inspector` for them anyway. The
//! server build therefore did not compile at all (`cannot find `ui` in `crate``).
//! Neither function is UI: one matches a stage asset to its open document, the other
//! walks `material:binding` on the composed stage. They belong here, where the
//! command layer and the Inspector can share them.

use bevy::prelude::*;
use lunco_doc::DocumentOrigin;
use lunco_doc_bevy::DocumentRegistry;
use lunco_materials::ParamValue;
use lunco_usd::document::UsdDocument;
use lunco_usd_bevy::{resolve_bound_shader, SdfPath, UsdPrimPath, UsdRead};

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
    let shader_path = bound_shader_prim(world, prim).ok_or_else(|| {
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
        .get_non_send::<lunco_usd_bevy::CanonicalStages>()
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
        if lunco_usd_bevy::author::parse_attribute_value(type_name, &literal).is_ok() {
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

#[cfg(test)]
mod tests {
    use super::canonical_shader_parameter_literal;
    use lunco_materials::ParamValue;

    #[test]
    fn shader_parameter_literal_uses_declared_usd_role_and_shape() {
        let color = ParamValue::Vec4([0.1, 0.2, 0.3, 1.0]);
        assert_eq!(
            canonical_shader_parameter_literal("color3f", &color).unwrap(),
            "(0.1, 0.2, 0.3)"
        );
        assert_eq!(
            canonical_shader_parameter_literal("color3f[]", &color).unwrap(),
            "[(0.1, 0.2, 0.3)]"
        );
        assert_eq!(
            canonical_shader_parameter_literal("float4", &color).unwrap(),
            "(0.1, 0.2, 0.3, 1)"
        );
    }

    #[test]
    fn shader_parameter_literal_rejects_declared_shape_mismatch() {
        let value = ParamValue::Vec2([0.1, 0.2]);
        assert!(canonical_shader_parameter_literal("float3", &value).is_err());
    }
}

/// The `UsdPreviewSurface` Shader prim bound to `prim`'s geometry, or `None` when it
/// has no material yet.
///
/// Walks `material:binding` → the Material's `outputs:surface` connection → the
/// Shader, on the LIVE canonical stage (building it from the asset's recipe if it has
/// not been built yet). Shared, because the two places that edit a look — the
/// Inspector panel and the `SetObjectProperty` command — must agree on WHERE the look
/// lives, or one of them will scribble `inputs:*` somewhere no other DCC reads it
/// back from.
pub fn bound_shader_prim(world: &mut World, prim: &UsdPrimPath) -> Option<String> {
    let id = prim.stage_handle.id();
    let mesh_sdf = SdfPath::new(&prim.path).ok()?;

    let recipe = world
        .get_resource::<Assets<lunco_usd_bevy::UsdStageAsset>>()
        .and_then(|stages| stages.get(&prim.stage_handle))
        .and_then(|a| a.recipe.clone());
    if let Some(mut canonical) = world.get_non_send_mut::<lunco_usd_bevy::CanonicalStages>() {
        if canonical.get(id).is_none() {
            if let Some(r) = recipe.as_ref() {
                canonical.get_or_build(id, r);
            }
        }
    }
    let canonical = world.get_non_send::<lunco_usd_bevy::CanonicalStages>()?;
    let view = canonical.get(id)?.view();
    resolve_bound_shader(&view, &mesh_sdf).map(|p| p.to_string())
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
        .get_resource::<Assets<lunco_usd_bevy::UsdStageAsset>>()
        .and_then(|stages| stages.get(&prim.stage_handle))
        .and_then(|a| a.recipe.clone());
    if let Some(mut canonical) = world.get_non_send_mut::<lunco_usd_bevy::CanonicalStages>() {
        if canonical.get(id).is_none() {
            if let Some(r) = recipe.as_ref() {
                canonical.get_or_build(id, r);
            }
        }
    }
    let Some(canonical) = world.get_non_send::<lunco_usd_bevy::CanonicalStages>() else {
        return Vec::new();
    };
    let Some(view) = canonical.get(id).map(|c| c.view()) else {
        return Vec::new();
    };
    view.stage()
        .prim(sdf)
        .api_schemas()
        .map(|v| v.iter().map(|s| s.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Resolve the editable USD document backing `entity`'s stage — the same
/// asset↔document match `apply_usd_path_attribute_change` needs, factored out so a
/// caller authoring a *sequence* of ops (the mount snap) resolves the doc once and
/// dispatches every op to it.
///
/// The stage-to-document binding is authoritative in
/// [`lunco_usd::twin_projection::DocBackedTwinScenes`]. File-origin matching is
/// retained for ordinary file-backed projections that are not Twin-mounted.
/// There is no active-viewport fallback: an entity without an explicit document
/// binding is not editable.
pub fn resolve_doc_for_entity(world: &World, entity: Entity) -> Option<lunco_doc::DocumentId> {
    let prim = world.get::<UsdPrimPath>(entity)?;
    let asset_server = world.get_resource::<AssetServer>()?;
    let asset_path = asset_server.get_path(prim.stage_handle.id())?;
    let path_str = asset_path.path().to_string_lossy().to_string();

    if let Some((name, rel)) = lunco_assets::split_twin_rel(&path_str) {
        if let Some(doc) = world
            .get_resource::<lunco_usd::twin_projection::DocBackedTwinScenes>()
            .and_then(|backed| backed.doc_for(name, rel))
        {
            return Some(doc);
        }
    }

    world
        .get_resource::<DocumentRegistry<UsdDocument>>()
        .and_then(|reg| {
            reg.ids().find(|id| {
                reg.host(*id).is_some_and(|h| match h.document().origin() {
                    DocumentOrigin::File { path, .. } => {
                        path.to_string_lossy().ends_with(&path_str)
                    }
                    _ => false,
                })
            })
        })
}
