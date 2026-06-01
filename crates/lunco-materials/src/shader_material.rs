//! General **`ShaderMaterial`** — one material, any WGSL, chosen per-instance.
//!
//! This is the *one* Rust material you ever need for custom shaders. After it
//! exists, new shaders are pure `.wgsl` files (+ optional USD properties) — no
//! Rust. It is engine-agnostic: nothing here is USD-specific. USD is just *one*
//! way to author it (see [`apply_shader_material_from_usd`]); the live
//! `SetObjectProperty` command is another.
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
//! }
//! @group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: ShaderParams;
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

use lunco_usd_bevy::UsdPrimPath;
use openusd::sdf::Path as SdfPath;
use crate::get_attribute_as_vec3;

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
        if let Some(fragment) = descriptor.fragment.as_mut() {
            fragment.shader = key.bind_group_data.shader.clone();
        }
        Ok(())
    }
}

/// Plugin: registers the material and the USD-authoring observer.
///
/// **No `load_internal_asset!`** — shaders load from `assets/shaders/*` by path,
/// so they hot-reload when edited (with asset watching enabled).
pub struct ShaderMaterialPlugin;

impl Plugin for ShaderMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ShaderMaterial>::default());
        app.add_observer(apply_shader_material_from_usd);
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
    match key {
        "colorA" | "color" => set_color(&mut m.color_a, value),
        "colorB" => set_color(&mut m.color_b, value),
        "colorC" => set_color(&mut m.color_c, value),
        "param0" => set_scalar(&mut m.params.x, value),
        "param1" => set_scalar(&mut m.params.y, value),
        "param2" => set_scalar(&mut m.params.z, value),
        "param3" => set_scalar(&mut m.params.w, value),
        "param4" => set_scalar(&mut m.params2.x, value),
        "param5" => set_scalar(&mut m.params2.y, value),
        "param6" => set_scalar(&mut m.params2.z, value),
        "param7" => set_scalar(&mut m.params2.w, value),
        _ => false,
    }
}

fn set_scalar(slot: &mut f32, value: &str) -> bool {
    match value.trim().parse::<f32>() {
        Ok(v) => { *slot = v; true }
        Err(_) => false,
    }
}

fn set_color(slot: &mut LinearRgba, value: &str) -> bool {
    let parts: Vec<f32> = value
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    if parts.len() >= 3 {
        *slot = LinearRgba::new(parts[0], parts[1], parts[2], 1.0);
        true
    } else {
        false
    }
}

/// Observer: applies [`ShaderMaterial`] the instant a prim's visuals are
/// instantiated, if it declares `primvars:materialType = "shader"` (or the
/// legacy alias `"usd_shader"`) and a `primvars:shaderPath`.
///
/// Observer-driven (fires once per prim on `Add<UsdVisualSynced>`) — no
/// per-frame poll, no whole-stage clone.
fn apply_shader_material_from_usd(
    trigger: On<Add, lunco_usd_bevy::UsdVisualSynced>,
    q: Query<&UsdPrimPath>,
    stages: Res<Assets<lunco_usd_bevy::UsdStageAsset>>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok(prim_path) = q.get(entity) else { return };
    let Some(stage) = stages.get(&prim_path.stage_handle) else { return };
    let Ok(sdf_path) = SdfPath::new(&prim_path.path) else { return };
    let reader = &*stage.reader;

    let mat_type: Option<String> = reader.prim_attribute_value(&sdf_path, "primvars:materialType");
    if !matches!(mat_type.as_deref(), Some("shader") | Some("usd_shader")) {
        return;
    }

    let Some(shader_path) = reader.prim_attribute_value::<String>(&sdf_path, "primvars:shaderPath")
    else {
        warn!("[shader] prim {} has materialType=shader but no primvars:shaderPath", prim_path.path);
        return;
    };

    // The shader is chosen by `primvars:shaderPath` (e.g. "shaders/wheel.wgsl"
    // or "shaders/balloon.wgsl"); generic colors/params come from primvars.
    let mut material = ShaderMaterial::default();
    read_authored_params(reader, &sdf_path, &mut material);
    material.shader = asset_server.load(&shader_path);

    let handle = materials.add(material);
    commands
        .entity(entity)
        .remove::<MeshMaterial3d<StandardMaterial>>()
        .insert(MeshMaterial3d(handle));
}

/// Reads `primvars:colorA/B/C` and `primvars:param0..7` into the material.
fn read_authored_params(reader: &openusd::usda::TextReader, sdf_path: &SdfPath, m: &mut ShaderMaterial) {
    for (attr, key) in [
        ("primvars:colorA", "colorA"),
        ("primvars:colorB", "colorB"),
        ("primvars:colorC", "colorC"),
    ] {
        if let Some(c) = get_attribute_as_vec3(reader, sdf_path, attr) {
            apply_param(m, key, &format!("{},{},{}", c.x, c.y, c.z));
        }
    }
    for i in 0..8 {
        let attr = format!("primvars:param{i}");
        let key = format!("param{i}");
        if let Some(v) = reader.prim_attribute_value::<f32>(sdf_path, &attr) {
            apply_param(m, &key, &v.to_string());
        } else if let Some(v) = reader.prim_attribute_value::<f64>(sdf_path, &attr) {
            apply_param(m, &key, &(v as f32).to_string());
        }
    }
}
