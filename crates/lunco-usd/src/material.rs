//! Lowering an "edit this thing's material" intent into a real UsdShade network.
//!
//! # Why this exists
//!
//! `inputs:*` is the **UsdShade** namespace. It belongs on a `Shader` prim, and
//! geometry reaches it through a `Material` and a `material:binding`
//! relationship:
//!
//! ```usda
//! def Sphere "Ball" { rel material:binding = </World/Looks/Ball_Mat> }
//! def Scope "Looks" {
//!     def Material "Ball_Mat" {
//!         token outputs:surface.connect = </World/Looks/Ball_Mat/Surface.outputs:surface>
//!         def Shader "Surface" {
//!             uniform token info:id = "UsdPreviewSurface"
//!             float inputs:metallic  = 1
//!             float inputs:roughness = 0.05
//!         }
//!     }
//! }
//! ```
//!
//! A `float inputs:metallic` authored **directly on the Sphere** is not valid
//! USD. No DCC writes it and none will round-trip it — the value is silently
//! dropped the first time the scene leaves this app.
//!
//! The Inspector used to author exactly that whenever you dragged the metallic
//! slider on a mesh with no material, and the importer had a matching fallback
//! that read it back — so the two bugs concealed each other and the scene looked
//! right until someone opened it in Houdini. Both halves are gone: the importer
//! reads shader inputs only (`lunco_usd_bevy::apply_standard_material`), and the
//! Inspector calls [`ensure_preview_surface_ops`] to *create the network it is
//! missing* instead of scribbling on the geometry.
//!
//! The one thing that legitimately lives on geometry is the `UsdGeomGprim`
//! display set — `primvars:displayColor` / `primvars:displayOpacity`. Those are
//! real geom attributes (a viewport hint for un-shaded prims) and are still both
//! read and written.

use crate::document::{LayerId, UsdOp};

/// The shader prim's name inside its `Material`.
const SURFACE: &str = "Surface";
/// The `Scope` that collects a scene's materials, by convention.
const LOOKS: &str = "Looks";

/// Ops that give `geom_path` a bound `UsdPreviewSurface`, plus the path of the
/// `Shader` prim to write `inputs:*` onto.
///
/// Idempotent: every op is a define-or-overwrite, so calling this for a prim
/// that already has this exact network re-authors the same values and changes
/// nothing. Callers should still prefer the existing binding when one is present
/// (`resolve_bound_shader`) — this is the "there is no material yet" path.
///
/// Returns `None` if `geom_path` is not an absolute prim path with a parent.
///
/// The network is anchored under the geom's **root prim** (`/World/Looks/…`,
/// `/SandboxScene/Looks/…`), which is inside the subtree the stage mounts. A
/// `Material` authored outside the mounted `defaultPrim` subtree composes into
/// the layer and is then never seen.
pub fn ensure_preview_surface_ops(geom_path: &str) -> Option<(Vec<UsdOp>, String)> {
    let geom = geom_path.strip_prefix('/')?;
    if geom.is_empty() {
        return None;
    }

    // `/World/Rovers/Wheel` → root `/World`, and a material name that is unique
    // per geom (`Rovers_Wheel_Mat`) so two prims never collide in one `Looks`.
    let mut parts = geom.split('/');
    let root_name = parts.next()?;
    let root = format!("/{root_name}");
    let rest: Vec<&str> = geom.split('/').skip(1).collect();
    let stem = if rest.is_empty() { root_name } else { &rest.join("_") };
    let mat_name = format!("{}_Mat", sanitize(stem));

    let looks = format!("{root}/{LOOKS}");
    let mat = format!("{looks}/{mat_name}");
    let shader = format!("{mat}/{SURFACE}");

    let root_layer = LayerId::root();
    let ops = vec![
        UsdOp::AddPrim {
            edit_target: root_layer.clone(),
            parent_path: root.clone(),
            name: LOOKS.into(),
            type_name: Some("Scope".into()),
            reference: None,
        },
        UsdOp::AddPrim {
            edit_target: root_layer.clone(),
            parent_path: looks,
            name: mat_name,
            type_name: Some("Material".into()),
            reference: None,
        },
        UsdOp::AddPrim {
            edit_target: root_layer.clone(),
            parent_path: mat.clone(),
            name: SURFACE.into(),
            type_name: Some("Shader".into()),
            reference: None,
        },
        // What makes the Shader a *preview surface* rather than an anonymous
        // node: consumers (this importer, Houdini, usdview) dispatch on `info:id`.
        UsdOp::SetAttribute {
            edit_target: root_layer.clone(),
            path: shader.clone(),
            name: "info:id".into(),
            type_name: "token".into(),
            value: "\"UsdPreviewSurface\"".into(),
        },
        // The Material's surface terminal ← the Shader's surface output. This is
        // the edge `resolve_bound_shader` walks; without it the Material is bound
        // but empty, and the geometry renders with the default look.
        UsdOp::SetConnection {
            edit_target: root_layer.clone(),
            path: mat.clone(),
            name: "outputs:surface".into(),
            type_name: "token".into(),
            sources: vec![format!("{shader}.outputs:surface")],
        },
        UsdOp::SetRelationship {
            edit_target: root_layer,
            path: geom_path.to_string(),
            name: "material:binding".into(),
            targets: vec![mat],
        },
    ];

    Some((ops, shader))
}

/// Coerce a prim-path fragment into a legal USD identifier (alphanumerics and
/// `_`, never leading with a digit).
fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(geom: &str) -> (Vec<String>, String) {
        let (ops, shader) = ensure_preview_surface_ops(geom).expect("ops");
        let described = ops
            .iter()
            .map(|op| match op {
                UsdOp::AddPrim { parent_path, name, type_name, .. } => {
                    format!("AddPrim {parent_path}/{name} : {}", type_name.clone().unwrap_or_default())
                }
                UsdOp::SetAttribute { path, name, .. } => format!("SetAttribute {path}.{name}"),
                UsdOp::SetConnection { path, name, sources, .. } => {
                    format!("SetConnection {path}.{name} -> {}", sources.join(","))
                }
                UsdOp::SetRelationship { path, name, targets, .. } => {
                    format!("SetRelationship {path}.{name} -> {}", targets.join(","))
                }
                _ => "?".into(),
            })
            .collect();
        (described, shader)
    }

    /// The whole contract: a bound Material, a UsdPreviewSurface Shader wired to
    /// its surface terminal, and the shader path the caller writes `inputs:*` to.
    #[test]
    fn builds_a_bound_preview_surface() {
        let (ops, shader) = paths("/World/Ball");
        assert_eq!(shader, "/World/Looks/Ball_Mat/Surface");
        assert_eq!(
            ops,
            vec![
                "AddPrim /World/Looks : Scope",
                "AddPrim /World/Looks/Ball_Mat : Material",
                "AddPrim /World/Looks/Ball_Mat/Surface : Shader",
                "SetAttribute /World/Looks/Ball_Mat/Surface.info:id",
                "SetConnection /World/Looks/Ball_Mat.outputs:surface -> /World/Looks/Ball_Mat/Surface.outputs:surface",
                "SetRelationship /World/Ball.material:binding -> /World/Looks/Ball_Mat",
            ]
        );
    }

    /// The `Looks` scope is anchored at the geom's ROOT prim, not at `/` — a
    /// Material outside the mounted `defaultPrim` subtree is never seen.
    #[test]
    fn nested_geom_anchors_material_at_the_root_prim() {
        let (_, shader) = paths("/SandboxScene/Rovers/Wheel");
        assert_eq!(shader, "/SandboxScene/Looks/Rovers_Wheel_Mat/Surface");
    }

    /// Distinct prims must not share a material — otherwise editing one recolours
    /// the other.
    #[test]
    fn distinct_geoms_get_distinct_materials() {
        let (_, a) = paths("/W/A/Body");
        let (_, b) = paths("/W/B/Body");
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_non_absolute_paths() {
        assert!(ensure_preview_surface_ops("World/Ball").is_none());
        assert!(ensure_preview_surface_ops("/").is_none());
    }
}
