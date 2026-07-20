//! Shader **discovery** and **starting templates** — render-free.
//!
//! The set of dynamic (`Material`-declaring) shaders the Inspector's shader-picker
//! can swap a prop to, plus the WGSL templates `CreateShader` starts from. Both are
//! plain strings + [`ParamSchema`](crate::dyn_params::ParamSchema) reflection —
//! nothing here names a material or a render pipeline, so it stays on the
//! render-free side of the boundary with `dyn_params` and `look`.
//!
//! Catalog *discovery* lives in ONE place — `lunco-sandbox-edit`'s
//! `maintain_catalogs`, which scans engine + Twin shaders via the shared
//! `lunco_assets::discovery` walk. This module only seeds the wasm-safe defaults in
//! [`ShaderCatalog::default`].

use bevy::prelude::*;

use crate::dyn_params::ParamSchema;

// ─────────────────────────────────────────────────────────────────────────
// Shader discovery — the set of dynamic (`Material`-declaring) shaders the
// Inspector's shader-picker can swap a prop to.
// ─────────────────────────────────────────────────────────────────────────

/// One pickable shader: its asset path (`shaders/foo.wgsl`) and display label.
#[derive(Clone, Debug)]
pub struct ShaderEntry {
    /// Asset path passed to `SetObjectProperty { property: "shader", .. }`.
    pub path: String,
    /// Title-cased label shown in the picker (`solar_panel` → `Solar Panel`).
    pub label: String,
}

/// The shaders the Inspector's picker offers. Seeded with the curated prop
/// shaders (so it is never empty and works on wasm, where there is no
/// filesystem to scan), then augmented on native by `lunco-sandbox-edit`'s
/// `maintain_catalogs` (via the shared `lunco_assets::discovery` walk).
#[derive(Resource, Clone, Debug)]
pub struct ShaderCatalog {
    pub entries: Vec<ShaderEntry>,
}

impl ShaderCatalog {
    /// Register a pickable shader by asset path (`shaders/foo.wgsl` or
    /// `twin://name/shaders/foo.wgsl`), deduped by path. The display label is
    /// derived from the file stem. Returns `true` if it was newly added.
    pub fn add(&mut self, path: impl Into<String>) -> bool {
        let path = path.into();
        if self.entries.iter().any(|e| e.path == path) {
            return false;
        }
        let stem = path
            .rsplit('/')
            .next()
            .unwrap_or(&path)
            .strip_suffix(".wgsl")
            .unwrap_or(&path);
        let label = humanize_shader_name(stem);
        self.entries.push(ShaderEntry { path, label });
        true
    }

    /// Unregister a shader by asset path. Returns `true` if it was present.
    pub fn remove(&mut self, path: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.path != path);
        self.entries.len() != before
    }
}

impl Default for ShaderCatalog {
    fn default() -> Self {
        // The prop-safe dynamic shaders that ship in `assets/shaders`. Terrain
        // shaders (regolith/terrain_shadow) are excluded: they declare engine
        // fields only the terrain entity's horizon system fills, so they would
        // render black on a prop (see [`is_prop_pickable_source`]).
        let entries = ["balloon", "solar_panel", "wheel"]
            .iter()
            .map(|n| ShaderEntry {
                path: format!("shaders/{n}.wgsl"),
                label: humanize_shader_name(n),
            })
            .collect();
        Self { entries }
    }
}

/// `solar_panel` → `Solar Panel`: filename stem → Title-Case display label.
fn humanize_shader_name(stem: &str) -> String {
    stem.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether WGSL `src` is safe to offer as a pickable prop shader: it declares a
/// `Material` struct (so the engine can reflect its params) and every
/// engine-filled field it declares is one a plain prop entity actually receives
/// (`prop_fillable` in the [engine-param registry][crate::engine_params]).
/// Terrain shaders declare engine fields like `sun_dir`/`hf_size` that only the
/// terrain entity's binder fills, so they would render black on a prop. Shaders
/// with no `Material` struct can't be driven by the dynamic system at all, so
/// they're rejected too.
///
/// The list of acceptable engine inputs is the registry's, not a literal here —
/// registering a new prop-fillable provider automatically widens this test.
pub fn is_prop_pickable_source(src: &str) -> bool {
    match ParamSchema::parse(src) {
        Some(schema) => crate::engine_params().prop_fillable(&schema),
        None => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Shader templates — PBR-compatible starting points for live shader creation
// (`CreateShader`). Each declares its own `Material` struct (so it is dynamic)
// and shades through the shared `lunco::pbr_lit::lit` helper (full lighting,
// shadows, ambient) so a generated shader looks correct under the sun.
// `__NAME__` is replaced with the shader's stem for the header comment.
// ─────────────────────────────────────────────────────────────────────────

const SOLID_TEMPLATE: &str = r#"//! __NAME__ — solid PBR material (generated template). Edit freely; saves
//! hot-reload, and the Inspector controls are reflected from the `//!@`
//! annotations + `Material` struct below.

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      base_color color "Base colour"
//!@default base_color 0.8,0.8,0.8
//!@ui      roughness  0 1  "Roughness"
//!@default roughness  0.6
//!@ui      metallic   0 1  "Metallic"
//!@default metallic   0.0
struct Material {
    base_color: vec3<f32>,
    roughness:  f32,
    metallic:   f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, mat.metallic, vec3<f32>(0.0));
}
"#;

const CHECKER_TEMPLATE: &str = r#"//! __NAME__ — procedural checker PBR material (generated template). The pattern
//! is computed from the object-space surface direction (seam-free on any mesh)
//! and shaded with full PBR lighting. Edit freely.

#import bevy_pbr::{forward_io::VertexOutput, mesh_functions}
#import lunco::pbr_lit::lit

//!@ui      color_a   color "Colour A"
//!@default color_a   0.85,0.2,0.2
//!@ui      color_b   color "Colour B"
//!@default color_b   0.1,0.1,0.12
//!@ui      tiles     1 64 "Tiles"
//!@default tiles     6
//!@ui      roughness 0 1  "Roughness"
//!@default roughness 0.6
struct Material {
    color_a:   vec3<f32>,
    tiles:     f32,
    color_b:   vec3<f32>,
    roughness: f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    // Object-space surface direction → seam-free lat/long coords (see balloon.wgsl).
    let m = mesh_functions::get_world_from_local(in.instance_index);
    let R = mat3x3<f32>(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz));
    let d = normalize(transpose(R) * normalize(in.world_normal));
    let u = atan2(d.z, d.x) * 0.15915494 + 0.5; // / (2*PI)
    let v = asin(clamp(d.y, -1.0, 1.0)) * 0.31830989 + 0.5; // / PI
    let t = mat.tiles;
    let cell = floor(u * t) + floor(v * t);
    let parity = cell - 2.0 * floor(cell * 0.5);
    let albedo = select(mat.color_a, mat.color_b, parity > 0.5);
    return lit(in, is_front, albedo, mat.roughness, 0.0, vec3<f32>(0.0));
}
"#;

const GRADIENT_TEMPLATE: &str = r#"//! __NAME__ — vertical gradient PBR material (generated template). Blends two
//! colours by surface normal (top vs bottom facing). Edit freely.

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      top       color "Top colour"
//!@default top       0.7,0.85,1.0
//!@ui      bottom    color "Bottom colour"
//!@default bottom    0.1,0.12,0.2
//!@ui      roughness 0 1  "Roughness"
//!@default roughness 0.5
struct Material {
    top:       vec3<f32>,
    roughness: f32,
    bottom:    vec3<f32>,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let t = clamp(n.y * 0.5 + 0.5, 0.0, 1.0);
    let albedo = mix(mat.bottom, mat.top, t);
    return lit(in, is_front, albedo, mat.roughness, 0.0, vec3<f32>(0.0));
}
"#;

const GLOW_TEMPLATE: &str = r#"//! __NAME__ — emissive glow PBR material (generated template). A lit base plus
//! a constant emissive term (×strength). Edit freely.

#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      base_color color "Base colour"
//!@default base_color 0.1,0.1,0.12
//!@ui      emissive   color "Glow colour"
//!@default emissive   1.0,0.6,0.1
//!@ui      strength   0 8  "Glow strength"
//!@default strength   2.0
//!@ui      roughness  0 1  "Roughness"
//!@default roughness  0.5
struct Material {
    base_color: vec3<f32>,
    strength:   f32,
    emissive:   vec3<f32>,
    roughness:  f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, mat.emissive * mat.strength);
}
"#;

const RIM_TEMPLATE: &str = r#"//! __NAME__ — rim / fresnel PBR material (generated template). A lit base with
//! a view-dependent rim glow on grazing angles. Edit freely.

#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}
#import lunco::pbr_lit::lit

//!@ui      base_color color "Base colour"
//!@default base_color 0.15,0.18,0.22
//!@ui      rim_color  color "Rim colour"
//!@default rim_color  0.4,0.8,1.0
//!@ui      rim_power  0.2 8 "Rim sharpness"
//!@default rim_power  3.0
//!@ui      roughness  0 1  "Roughness"
//!@default roughness  0.4
struct Material {
    base_color: vec3<f32>,
    rim_power:  f32,
    rim_color:  vec3<f32>,
    roughness:  f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position.xyz - in.world_position.xyz);
    let rim = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), max(mat.rim_power, 0.001));
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, mat.rim_color * rim);
}
"#;

/// The built-in template ids + display labels, for a "New shader" UI dropdown.
pub fn shader_template_kinds() -> &'static [(&'static str, &'static str)] {
    &[
        ("solid", "Solid"),
        ("checker", "Checker"),
        ("gradient", "Gradient"),
        ("glow", "Emissive Glow"),
        ("rim", "Rim / Fresnel"),
    ]
}

/// WGSL source for a built-in starting template, with `title` substituted into
/// the header comment. `kind` selects the template (see [`shader_template_kinds`]);
/// anything unknown (incl. empty) falls back to the flat solid material. All
/// templates are PBR-lit (`lunco::pbr_lit::lit`) and prop-pickable.
pub fn shader_template(kind: &str, title: &str) -> String {
    let body = match kind.trim() {
        "checker" => CHECKER_TEMPLATE,
        "gradient" => GRADIENT_TEMPLATE,
        "glow" => GLOW_TEMPLATE,
        "rim" => RIM_TEMPLATE,
        _ => SOLID_TEMPLATE,
    };
    body.replace("__NAME__", title)
}
