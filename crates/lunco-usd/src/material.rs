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
/// The `Scope` that collects a scene's *look* materials, by convention.
const LOOKS: &str = "Looks";
/// The `Scope` that collects a scene's *physics* materials. Separate from
/// [`LOOKS`] on purpose — see [`ensure_physics_material_ops`].
const PHYSICS_MATERIALS: &str = "PhysicsMaterials";

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

/// Ops that give `geom_path` a bound **physics** surface: a `Material` carrying
/// `UsdPhysicsMaterialAPI`, bound through the purpose-specific
/// `material:binding:physics`.
///
/// # Why this is a SEPARATE Material from the look
///
/// USD *permits* one `Material` prim to carry both a `UsdPreviewSurface` and an
/// applied `PhysicsMaterialAPI` — that is precisely why binding resolution falls
/// back from `material:binding:physics` to `material:binding` (openusd's own
/// `physics::tokens` documents the fallback). So a merged material is legal.
///
/// We still author them apart, and only *share the code*:
///
/// - A look and a surface are not the same authoring decision. Ice and glass look
///   alike and grip nothing alike; regolith and concrete grip alike and look
///   nothing alike. Forcing one prim couples two axes that authors vary
///   independently, and there is no way back out of it once scenes are written.
/// - Physics materials are a small shared vocabulary (a handful of surfaces reused
///   across a whole scene), while looks are per-object. They have different
///   cardinality, so one prim per geom is the wrong shape for physics.
///
/// What IS shared is the part that should never diverge: binding *resolution* —
/// namespace inheritance and the purpose fallback — which lives once in
/// [`lunco_usd_bevy::resolve_bound_material`] and serves both. A scene that DOES
/// merge them still resolves correctly through that fallback, so we read the
/// legal form even though we don't author it.
pub fn ensure_physics_material_ops(
    geom_path: &str,
    name: &str,
    dynamic_friction: f32,
    static_friction: f32,
    restitution: Option<f32>,
) -> Option<Vec<UsdOp>> {
    let geom = geom_path.strip_prefix('/')?;
    let root_name = geom.split('/').next()?;
    if root_name.is_empty() {
        return None;
    }
    let root = format!("/{root_name}");
    let scope = format!("{root}/{PHYSICS_MATERIALS}");
    let mat_name = sanitize(name);
    let mat = format!("{scope}/{mat_name}");

    let root_layer = LayerId::root();
    let mut ops = vec![
        UsdOp::AddPrim {
            edit_target: root_layer.clone(),
            parent_path: root,
            name: PHYSICS_MATERIALS.into(),
            type_name: Some("Scope".into()),
            reference: None,
        },
        UsdOp::AddPrim {
            edit_target: root_layer.clone(),
            parent_path: scope,
            name: mat_name,
            type_name: Some("Material".into()),
            reference: None,
        },
        UsdOp::SetApiSchemas {
            edit_target: root_layer.clone(),
            path: mat.clone(),
            schemas: vec!["PhysicsMaterialAPI".into()],
        },
    ];

    let mut attr = |name: &str, value: String| {
        ops.push(UsdOp::SetAttribute {
            edit_target: root_layer.clone(),
            path: mat.clone(),
            name: name.into(),
            type_name: "float".into(),
            value,
        });
    };
    // Dynamic and static friction stay distinct — USD models them separately and
    // so does the solver.
    attr("physics:dynamicFriction", dynamic_friction.to_string());
    attr("physics:staticFriction", static_friction.to_string());
    if let Some(r) = restitution {
        attr("physics:restitution", r.to_string());
    }

    ops.push(UsdOp::SetRelationship {
        edit_target: root_layer,
        path: geom_path.to_string(),
        name: "material:binding:physics".into(),
        targets: vec![mat],
    });
    Some(ops)
}

/// The `UsdPreviewSurface` input a LunCoSim PBR look key maps to, as
/// `(attribute, USD type)` — or `None` when USD's surface model has no equivalent.
///
/// The single home for the mapping, so the Inspector's material editor and the
/// `SetObjectProperty` command cannot disagree about what "roughness" means. This
/// is the crate the mapping belongs in (not the editor) precisely so every crate
/// that edits a look authors the SAME USD.
///
/// `None` is a real answer, not a gap:
/// - `unlit` has no `UsdPreviewSurface` input, and should not: it is not a claim
///   about a surface but about the geometry's *role* ("this is a symbol, not a
///   surface" — trajectory lines, overlays, labels). It is render-only intent, set
///   from Rust, and no scene authors it. A genuinely unlit *surface* is spelled the
///   USD way: emissive-only (`diffuseColor` 0, `emissiveColor` C, `specularColor` 0).
/// - `double_sided` is NOT a surface input at all — it is `uniform bool
///   doubleSided` on `UsdGeomGprim`, i.e. a property of the *geometry*, not the
///   material. It is handled by the caller, on the geom prim.
///
/// There is no `reflectance` key. `UsdPreviewSurface` has no such input: specular
/// strength is `inputs:ior`, and Bevy's `reflectance` is a remap of the same
/// physical quantity (F₀), derived from `ior` in `lunco-render-bevy`. Mapping the
/// two is exact, not lossy — `ior` 1.5 and `reflectance` 0.5 both mean F₀ = 0.04 —
/// so there is nothing to preserve by keeping a second name for it.
pub fn preview_surface_input(key: &str) -> Option<(&'static str, &'static str)> {
    Some(match key {
        "base_color" => ("inputs:diffuseColor", "color3f"),
        "emissive" => ("inputs:emissiveColor", "color3f"),
        "metallic" => ("inputs:metallic", "float"),
        "roughness" | "perceptual_roughness" => ("inputs:roughness", "float"),
        "alpha" | "opacity" => ("inputs:opacity", "float"),
        "ior" => ("inputs:ior", "float"),
        _ => return None,
    })
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

    /// A physics material is its OWN `Material` prim, in its own scope, bound for
    /// the `physics` purpose — NOT merged into the geom's look material. Looks are
    /// per-object; surfaces are a small shared vocabulary. (USD would permit the
    /// merged form — that is what the purpose fallback is for — and we still READ
    /// it; we just don't author it. See `ensure_physics_material_ops`.)
    #[test]
    fn physics_material_is_separate_from_the_look() {
        let ops = ensure_physics_material_ops("/World/Ground", "Regolith", 0.9, 1.0, Some(0.1))
            .expect("ops");

        assert!(
            ops.iter().any(|o| matches!(o, UsdOp::SetApiSchemas { path, schemas, .. }
                if path == "/World/PhysicsMaterials/Regolith"
                    && schemas == &["PhysicsMaterialAPI".to_string()])),
            "PhysicsMaterialAPI applies to a Material in the PhysicsMaterials scope"
        );

        let bindings: Vec<(&str, &str)> = ops
            .iter()
            .filter_map(|o| match o {
                UsdOp::SetRelationship { name, targets, .. } => {
                    Some((name.as_str(), targets[0].as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            bindings,
            vec![(
                "material:binding:physics",
                "/World/PhysicsMaterials/Regolith"
            )],
            "bound for the physics purpose only — the look binding is untouched"
        );

        // Dynamic and static friction survive as DISTINCT values.
        let friction: Vec<(&str, &str)> = ops
            .iter()
            .filter_map(|o| match o {
                UsdOp::SetAttribute { name, value, .. } if name.starts_with("physics:") => {
                    Some((name.as_str(), value.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            friction,
            vec![
                ("physics:dynamicFriction", "0.9"),
                ("physics:staticFriction", "1"),
                ("physics:restitution", "0.1"),
            ]
        );
    }
}
