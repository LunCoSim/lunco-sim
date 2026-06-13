//! General **`ShaderMaterial`** — one material, any WGSL, chosen per-instance.
//!
//! This is the *one* Rust material you ever need for custom shaders. After it
//! exists, new shaders are pure `.wgsl` files (+ optional USD properties) — no
//! Rust. It is engine-agnostic: nothing here is USD-specific. USD is just *one*
//! way to author it (the USD→material binding lives in `lunco-usd-sim`'s
//! `apply_usd_shader_materials`, deterministically ordered so it can never race
//! a downstream consumer); the live `SetObjectProperty` command is another.
//!
//! ## Why one material can drive many shaders
//! Bevy resolves `fragment_shader()` per material *type*, not per instance. To let
//! each object pick its own `.wgsl`, we:
//! 1. Store the per-object `Handle<Shader>` in the material (loaded from an
//!    **asset path**, so editing the `.wgsl` on disk hot-reloads).
//! 2. Carry that handle into the pipeline key via `#[bind_group_data]`
//!    (`Handle<Shader>` is `Clone + Eq + Hash` — all a plain `Material` key needs;
//!    `ExtendedMaterial` would additionally require `Copy`, which a handle isn't,
//!    so we deliberately use a *plain* `Material`).
//! 3. Override [`Material::specialize`] to overwrite `descriptor.fragment.shader`
//!    with the per-instance handle.
//!
//! ## Uniform contract (every `.wgsl` sees this at `@binding(0)`)
//! ```wgsl
//! struct ShaderParams {
//!     color_a: vec4<f32>,
//!     color_b: vec4<f32>,
//!     color_c: vec4<f32>,
//!     params:  vec4<f32>,  // generic scalars 0..3
//!     params2: vec4<f32>,  // generic scalars 4..7
//!     engine:  vec4<f32>,  // engine-written (see ShaderMaterial::engine; optional to declare)
//!     engine2: vec4<f32>,  // engine-written, terrain shaders only
//! }
//! @group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: ShaderParams;
//! ```
//! Terrain shaders additionally declare the heightfield (non-filterable,
//! fetched with `textureLoad`):
//! ```wgsl
//! @group(#{MATERIAL_BIND_GROUP}) @binding(1) var height_map: texture_2d<f32>;
//! ```
//! A plain `Material` is unlit by default; the `.wgsl` returns its own colour
//! (it may shade using `VertexOutput.world_normal` if it wants form).

use bevy::prelude::*;
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::Shader;

/// A general custom-shader material. Field order/types must match the
/// `ShaderParams` struct in WGSL exactly (all `vec4` → no std140 padding).
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(ShaderKey)]
pub struct ShaderMaterial {
    #[uniform(0)]
    pub color_a: LinearRgba,
    #[uniform(0)]
    pub color_b: LinearRgba,
    #[uniform(0)]
    pub color_c: LinearRgba,
    /// Generic scalars 0..3 (`param0`..`param3`).
    #[uniform(0)]
    pub params: Vec4,
    /// Generic scalars 4..7 (`param4`..`param7`).
    #[uniform(0)]
    pub params2: Vec4,
    /// Engine-written channel, **not** author-settable. Semantics depend on
    /// what the shader is for (the horizon-shadow system writes the right
    /// one per entity):
    /// - prop shaders (wheel/balloon/panel): `x` = sun visibility 0..1 —
    ///   multiply your output by `mat.engine.x`;
    /// - terrain shaders (regolith/terrain_shadow): `xyz` = terrain-local
    ///   to-sun direction, `w` = tan of the sun's angular radius, for the
    ///   per-pixel heightfield shadow ray-march.
    /// Declared after the authored fields so shaders written against the
    /// 5-vec4 contract keep working (a shader-side struct may be a prefix
    /// of the uniform buffer).
    #[uniform(0)]
    pub engine: Vec4,
    /// Second engine channel (terrain shaders):
    /// `(size_x, size_z, heightfield_resolution, 0)`.
    #[uniform(0)]
    pub engine2: Vec4,
    /// Terrain heightfield (R32Float world heights) for ray-marched sun
    /// shadows, written by `lunco-environment`'s horizon system. Sampled
    /// with `textureLoad` — R32Float is non-filterable in core WebGPU.
    /// `None` binds Bevy's fallback image; shaders that don't declare the
    /// binding are unaffected.
    #[texture(1, sample_type = "float", filterable = false)]
    pub height_map: Option<Handle<Image>>,
    /// Per-instance fragment shader. **Not** a bind-group resource — it drives
    /// pipeline specialization (see [`ShaderMaterial::specialize`]) and is kept
    /// as a strong handle so the asset stays loaded.
    pub shader: Handle<Shader>,
}

impl Default for ShaderMaterial {
    fn default() -> Self {
        Self {
            color_a: LinearRgba::new(0.95, 0.85, 0.10, 1.0),
            color_b: LinearRgba::new(0.10, 0.10, 0.12, 1.0),
            color_c: LinearRgba::new(0.90, 0.15, 0.15, 1.0),
            params: Vec4::ZERO,
            params2: Vec4::ZERO,
            engine: Vec4::ONE,
            engine2: Vec4::ZERO,
            height_map: None,
            shader: Handle::default(),
        }
    }
}

/// Pipeline key carrying the per-instance shader handle into `specialize`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ShaderKey {
    shader: Handle<Shader>,
}

impl From<&ShaderMaterial> for ShaderKey {
    fn from(m: &ShaderMaterial) -> Self {
        Self { shader: m.shader.clone() }
    }
}

impl Material for ShaderMaterial {
    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // `specialize` runs for *every* pass that uses this material, including
        // the depth/normal **prepass**. Our per-instance `.wgsl` only defines a
        // main-pass `@fragment` (it returns a colour); it has none of the
        // prepass entry points/outputs. Overwriting the prepass fragment shader
        // with it produces an invalid `prepass_pipeline` — tolerated by Vulkan,
        // rejected by WebGPU's stricter validation (frames dropped in-browser).
        // So only swap the fragment shader for non-prepass pipelines; let the
        // prepass keep Bevy's default shader (it only needs depth/normals).
        let is_prepass = descriptor
            .label
            .as_ref()
            .is_some_and(|l| l.contains("prepass"));
        if !is_prepass {
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader = key.bind_group_data.shader.clone();
            }
        }
        Ok(())
    }
}

/// Plugin: registers the [`ShaderMaterial`] render pipeline.
///
/// **No `load_internal_asset!`** — shaders load from `assets/shaders/*` by path,
/// so they hot-reload when edited (with asset watching enabled).
///
/// This plugin is intentionally *only* the engine-agnostic material. Authoring
/// from USD is a separate, deterministically-ordered system in `lunco-usd-sim`
/// (`apply_usd_shader_materials`) so material application can never race a
/// downstream consumer (e.g. the wheel physics/visual split).
pub struct ShaderMaterialPlugin;

/// Keeps the shared `lunco::horizon` WGSL module (heightfield shadow
/// ray-march) loaded so `#import lunco::horizon::sun_visibility` resolves in
/// any per-instance shader.
#[derive(Resource)]
pub struct HorizonMarchModule(#[allow(dead_code)] Handle<bevy::shader::Shader>);

impl Plugin for ShaderMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ShaderMaterial>::default());
        let module = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/horizon_march.wgsl");
        app.insert_resource(HorizonMarchModule(module));
    }
}

/// Builds a [`ShaderMaterial`] from a shader handle + a template of uniforms.
pub fn build_shader_material(shader: Handle<Shader>, mut material: ShaderMaterial) -> ShaderMaterial {
    material.shader = shader;
    material
}

/// Applies one named property to a material's generic uniforms. Returns `true`
/// if `key` was recognised. Shared by the USD-authoring observer and the live
/// `SetObjectProperty` command so both speak the same property vocabulary.
///
/// Recognised keys:
/// - `colorA` / `color`, `colorB`, `colorC` → comma-separated `r,g,b`
/// - `param0`..`param7` → single float (param4..7 spill into `params2`)
pub fn apply_param(m: &mut ShaderMaterial, key: &str, value: &str) -> bool {
    if let Some(slot) = scalar_slot(m, key) {
        return match value.trim().parse::<f32>() {
            Ok(v) => { *slot = v; true }
            Err(_) => false,
        };
    }
    if let Some(slot) = color_slot(m, key) {
        let parts: Vec<f32> =
            value.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
        if parts.len() >= 3 {
            *slot = LinearRgba::new(parts[0], parts[1], parts[2], 1.0);
            return true;
        }
    }
    false
}

/// Mutable handle to the `f32` uniform a scalar `key` (`param0`..`param7`)
/// addresses — the single place the name→slot mapping lives.
fn scalar_slot<'a>(m: &'a mut ShaderMaterial, key: &str) -> Option<&'a mut f32> {
    Some(match key {
        "param0" => &mut m.params.x,
        "param1" => &mut m.params.y,
        "param2" => &mut m.params.z,
        "param3" => &mut m.params.w,
        "param4" => &mut m.params2.x,
        "param5" => &mut m.params2.y,
        "param6" => &mut m.params2.z,
        "param7" => &mut m.params2.w,
        _ => return None,
    })
}

/// Mutable handle to the colour uniform a `key` (`colorA`/`colorB`/`colorC`)
/// addresses.
fn color_slot<'a>(m: &'a mut ShaderMaterial, key: &str) -> Option<&'a mut LinearRgba> {
    Some(match key {
        "colorA" | "color" => &mut m.color_a,
        "colorB" => &mut m.color_b,
        "colorC" => &mut m.color_c,
        _ => return None,
    })
}

/// Reads the current scalar uniform for `key`, or `None` if `key` isn't a
/// scalar param. (Takes `&mut` only to reuse [`scalar_slot`]; doesn't mutate.)
pub fn get_scalar(m: &ShaderMaterial, key: &str) -> Option<f32> {
    match key {
        "param0" => Some(m.params.x),
        "param1" => Some(m.params.y),
        "param2" => Some(m.params.z),
        "param3" => Some(m.params.w),
        "param4" => Some(m.params2.x),
        "param5" => Some(m.params2.y),
        "param6" => Some(m.params2.z),
        "param7" => Some(m.params2.w),
        _ => None,
    }
}

/// Writes a scalar uniform by `key`. Returns `false` for unknown keys.
pub fn set_scalar_value(m: &mut ShaderMaterial, key: &str, v: f32) -> bool {
    match scalar_slot(m, key) {
        Some(slot) => { *slot = v; true }
        None => false,
    }
}

/// Reads the current colour uniform for `key` as linear RGB.
pub fn get_color(m: &ShaderMaterial, key: &str) -> Option<[f32; 3]> {
    let c = match key {
        "colorA" | "color" => m.color_a,
        "colorB" => m.color_b,
        "colorC" => m.color_c,
        _ => return None,
    };
    Some([c.red, c.green, c.blue])
}

/// Writes a colour uniform by `key` (linear RGB). Returns `false` for unknown keys.
pub fn set_color_value(m: &mut ShaderMaterial, key: &str, rgb: [f32; 3]) -> bool {
    match color_slot(m, key) {
        Some(slot) => { *slot = LinearRgba::new(rgb[0], rgb[1], rgb[2], 1.0); true }
        None => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Parameter manifest — names, ranges, and defaults for the generic uniforms.
//
// `param0`..`param7` and the three colours are *generic* slots; their meaning
// is shader-specific (documented in each `.wgsl` header). This manifest is the
// machine-readable mirror of those headers, so editors (the Inspector) and the
// API can present named, range-bounded controls instead of raw `param3`
// sliders. One source of truth, keyed by shader file name.
// ─────────────────────────────────────────────────────────────────────────

/// How one shader parameter should be presented and bounded.
#[derive(Clone, Copy, Debug)]
pub enum ShaderParamKind {
    /// Continuous slider. `default` mirrors the shader's built-in fallback
    /// (the `select(.., d, x < 1e-4)` idiom): a stored 0 means "unset", so
    /// editors should display `default` until the user moves the control.
    Scalar { min: f32, max: f32, default: f32, log: bool },
    /// Integer-valued slider (counts: spokes, cells, …). Same 0-means-unset
    /// rule as [`ShaderParamKind::Scalar`].
    Int { min: i32, max: i32, default: i32 },
    /// Linear-RGB colour. `default` is shown when the stored colour still
    /// equals the [`ShaderMaterial`] prop-yellow sentinel (an unauthored
    /// `colorA`), matching how shaders substitute their own default.
    Color { default: [f32; 3] },
    /// Unknown shader: a free numeric field with no fixed range.
    Free,
}

/// One named, presentable shader parameter.
#[derive(Clone, Copy, Debug)]
pub struct ShaderParamDesc {
    /// The generic key `apply_param`/`get_scalar`/`get_color` speak
    /// (`param0`..`param7`, `colorA`/`colorB`/`colorC`).
    pub key: &'static str,
    /// Human label for the control.
    pub label: &'static str,
    pub kind: ShaderParamKind,
}

/// The prop-yellow `colorA` default that shaders treat as "unauthored".
pub const PROP_YELLOW: [f32; 3] = [0.95, 0.85, 0.10];

macro_rules! sc {
    ($key:literal, $label:literal, $min:literal, $max:literal, $def:literal) => {
        ShaderParamDesc { key: $key, label: $label,
            kind: ShaderParamKind::Scalar { min: $min, max: $max, default: $def, log: false } }
    };
    ($key:literal, $label:literal, $min:literal, $max:literal, $def:literal, log) => {
        ShaderParamDesc { key: $key, label: $label,
            kind: ShaderParamKind::Scalar { min: $min, max: $max, default: $def, log: true } }
    };
}
macro_rules! int {
    ($key:literal, $label:literal, $min:literal, $max:literal, $def:literal) => {
        ShaderParamDesc { key: $key, label: $label,
            kind: ShaderParamKind::Int { min: $min, max: $max, default: $def } }
    };
}
macro_rules! col {
    ($key:literal, $label:literal, $r:literal, $g:literal, $b:literal) => {
        ShaderParamDesc { key: $key, label: $label,
            kind: ShaderParamKind::Color { default: [$r, $g, $b] } }
    };
}

const REGOLITH: &[ShaderParamDesc] = &[
    col!("colorA", "Albedo", 0.17, 0.17, 0.17),
    sc!("param0", "Macro clump scale (/m)", 1.0, 20.0, 8.0),
    sc!("param2", "Macro bump strength", 0.0, 0.3, 0.06),
    sc!("param5", "Mid hummock scale (/m)", 0.02, 1.0, 0.15),
    sc!("param6", "Mid hummock strength", 0.0, 1.5, 0.6),
    sc!("param1", "Fine grain scale (/m)", 50.0, 400.0, 180.0),
    sc!("param3", "Fine grain strength", 0.0, 0.1, 0.025),
    sc!("param4", "Roughness mix", 0.0, 1.0, 0.35),
    sc!("param7", "Albedo mottle", 0.0, 0.6, 0.22),
];

const WHEEL: &[ShaderParamDesc] = &[
    int!("param0", "Spoke count", 1, 16, 6),
    int!("param1", "Tread lugs", 4, 64, 24),
    sc!("param2", "Spoke width", 0.0, 1.0, 0.35),
    int!("param3", "Marker spokes", 0, 8, 1),
    col!("colorA", "Spoke / rim", 0.7, 0.7, 0.75),
    col!("colorB", "Tire", 0.1, 0.1, 0.12),
    col!("colorC", "Marker / hub", 0.9, 0.15, 0.15),
];

const BALLOON: &[ShaderParamDesc] = &[
    int!("param0", "Wedge count", 2, 24, 8),
    int!("param1", "Band count", 2, 24, 6),
    int!("param3", "Marker wedges", 0, 12, 0),
    col!("colorA", "Cell A", 0.9, 0.9, 0.9),
    col!("colorB", "Cell B", 0.1, 0.1, 0.12),
    col!("colorC", "Marker / poles", 0.9, 0.15, 0.15),
];

const SOLAR_PANEL: &[ShaderParamDesc] = &[
    int!("param0", "Cell rows", 1, 32, 12),
    int!("param1", "Cell cols", 1, 32, 6),
    sc!("param2", "Cell gap", 0.0, 0.1, 0.02),
    sc!("param3", "Bus width", 0.0, 0.05, 0.004),
    sc!("param4", "Frame border", 0.0, 0.2, 0.04),
    col!("colorA", "Cell", 0.1, 0.12, 0.3),
    col!("colorB", "Bus line", 0.7, 0.7, 0.75),
    col!("colorC", "Frame", 0.2, 0.2, 0.22),
];

/// Generic fallback for an unrecognised shader: all eight scalars + three
/// colours as free controls.
const GENERIC: &[ShaderParamDesc] = &[
    ShaderParamDesc { key: "param0", label: "param0", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param1", label: "param1", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param2", label: "param2", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param3", label: "param3", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param4", label: "param4", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param5", label: "param5", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param6", label: "param6", kind: ShaderParamKind::Free },
    ShaderParamDesc { key: "param7", label: "param7", kind: ShaderParamKind::Free },
    col!("colorA", "Color A", 0.95, 0.85, 0.10),
    col!("colorB", "Color B", 0.10, 0.10, 0.12),
    col!("colorC", "Color C", 0.90, 0.15, 0.15),
];

/// Named, range-bounded parameters for a shader, looked up by its asset path
/// (matched on the file name, so `lunco://shaders/regolith.wgsl` and
/// `shaders/regolith.wgsl` both resolve). Unknown or `None` paths fall back to
/// the [`GENERIC`] manifest so every `ShaderMaterial` is at least raw-editable.
pub fn shader_param_manifest(path: Option<&str>) -> &'static [ShaderParamDesc] {
    let Some(path) = path else { return GENERIC };
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match file {
        "regolith.wgsl" => REGOLITH,
        "wheel.wgsl" => WHEEL,
        "balloon.wgsl" => BALLOON,
        "solar_panel.wgsl" => SOLAR_PANEL,
        _ => GENERIC,
    }
}
