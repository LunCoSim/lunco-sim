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
//! ## Uniform contract — each `.wgsl` declares its OWN params (`@binding(0)`)
//! Every shader declares a `Material` struct with real field names; the engine
//! reflects the layout (field names → std140 offsets) and the `//!@` annotation
//! comments (UI ranges, defaults, engine-filled fields) straight out of the
//! source (see [`crate::dyn_params`]). Values are stored by name and packed into
//! the opaque [`raw`](ShaderMaterial::raw) block at the reflected offsets.
//! ```wgsl
//! //!@ui      albedo  color "Albedo"
//! //!@default albedo  0.5,0.5,0.5
//! //!@engine  sun_vis              // Rust-filled each frame
//! struct Material { albedo: vec3<f32>, sun_vis: f32 }
//! @group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;
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
use bevy::shader::{Shader, Source as ShaderSource};
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crate::dyn_params::{self, ParamSchema, ParamType, ParamValue};

/// A general custom-shader material whose parameters are **dynamic**: each
/// `.wgsl` declares its own `Material` uniform struct (real field names), the
/// engine reflects that layout from the shader source ([`ParamSchema`]),
/// values are stored by name in [`values`](Self::values), and packed into a
/// fixed 256-byte opaque block ([`raw`](Self::raw)) at the reflected offsets.
/// No parameter names/ranges/defaults are hardcoded in Rust. A fresh material
/// has an empty schema (packs all-zero) until the reflect system derives one
/// from its shader source.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
#[bind_group_data(ShaderKey)]
pub struct ShaderMaterial {
    /// Opaque uniform block (256 bytes). Its field layout is the *shader's* —
    /// each `.wgsl` reinterprets these bytes through its own `Material`
    /// struct. Built from [`values`](Self::values) via
    /// [`schema`](Self::schema); never edit directly, use `set*`.
    #[uniform(0)]
    pub raw: [Vec4; dyn_params::BLOCK_VEC4S],
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
    /// Reflected parameter layout for [`shader`](Self::shader). Defaults to the
    /// legacy fixed layout; the reflect system upgrades it once the shader
    /// source is available. Cheap shared `Arc`. Not a bind-group resource.
    pub schema: Arc<ParamSchema>,
    /// Authored/live parameter values by name; packed into `raw` on change.
    /// Not a bind-group resource.
    pub values: BTreeMap<String, ParamValue>,
}

/// The shared empty schema (created once). A fresh material carries this until
/// the reflect system derives a real schema from its shader source.
fn empty_schema_arc() -> Arc<ParamSchema> {
    static EMPTY: OnceLock<Arc<ParamSchema>> = OnceLock::new();
    EMPTY.get_or_init(|| Arc::new(ParamSchema { fields: Vec::new(), size: 0 })).clone()
}

impl Default for ShaderMaterial {
    fn default() -> Self {
        // Empty schema + no values: packs all-zero until the reflect system
        // derives the real layout from the shader. The material doesn't render
        // before its shader loads (the pipeline can't build), so there's no
        // black flash; authored/engine values are stored by name and applied
        // the moment the schema lands.
        Self {
            raw: [Vec4::ZERO; dyn_params::BLOCK_VEC4S],
            height_map: None,
            shader: Handle::default(),
            schema: empty_schema_arc(),
            values: BTreeMap::new(),
        }
    }
}

impl ShaderMaterial {
    /// Rebuilds the GPU uniform block from `values` (+ schema defaults).
    pub fn repack(&mut self) {
        self.raw = self.schema.pack(&self.values);
    }
    /// Assigns a reflected schema (from the shader source) and repacks.
    pub fn set_schema(&mut self, schema: Arc<ParamSchema>) {
        self.schema = schema;
        self.repack();
    }
    /// Sets a parameter by name and repacks.
    pub fn set(&mut self, name: &str, v: ParamValue) {
        self.values.insert(name.to_string(), v);
        self.repack();
    }
    /// Current value for `name` (or its schema default).
    pub fn get(&self, name: &str) -> Option<ParamValue> {
        self.values
            .get(name)
            .copied()
            .or_else(|| self.schema.field(name).and_then(|f| f.default))
    }
    pub fn set_scalar(&mut self, name: &str, v: f32) {
        self.set(name, ParamValue::F32(v));
    }
    pub fn get_scalar(&self, name: &str) -> Option<f32> {
        match self.get(name)? {
            ParamValue::F32(v) => Some(v),
            ParamValue::I32(v) => Some(v as f32),
            ParamValue::U32(v) => Some(v as f32),
            _ => None,
        }
    }
    pub fn set_color(&mut self, name: &str, rgb: [f32; 3]) {
        self.set(name, ParamValue::Vec4([rgb[0], rgb[1], rgb[2], 1.0]));
    }
    pub fn get_color(&self, name: &str) -> Option<[f32; 3]> {
        match self.get(name)? {
            ParamValue::Vec4(v) => Some([v[0], v[1], v[2]]),
            ParamValue::Vec3(v) => Some(v),
            _ => None,
        }
    }
    pub fn set_vec4(&mut self, name: &str, v: Vec4) {
        self.set(name, ParamValue::Vec4(v.to_array()));
    }
    pub fn get_vec4(&self, name: &str) -> Option<Vec4> {
        match self.get(name)? {
            ParamValue::Vec4(v) => Some(Vec4::from_array(v)),
            _ => None,
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
        app.init_resource::<ShaderSchemas>();
        // Reflect each shader's `Material` struct → per-material `ParamSchema`.
        app.add_systems(Update, reflect_shader_schemas);
        let module = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/horizon_march.wgsl");
        app.insert_resource(HorizonMarchModule(module));
    }
}

/// Builds a [`ShaderMaterial`] from a shader handle + a template (preserves
/// the template's named `values`/`schema` so swapping the `.wgsl` keeps tuned
/// params). The reflect system re-derives the schema from the new shader.
pub fn build_shader_material(shader: Handle<Shader>, mut material: ShaderMaterial) -> ShaderMaterial {
    material.shader = shader;
    material
}

/// Sets one named property from a string (the USD-authoring + `SetObjectProperty`
/// text vocabulary). Resolves the field's type from the material's reflected
/// schema, parses, and stores by name. Returns `true` if the value parsed.
pub fn apply_param(m: &mut ShaderMaterial, key: &str, value: &str) -> bool {
    // Type from the reflected schema if known; else infer from the value's
    // arity (so values authored before the shader is reflected still store —
    // packing applies them at the reflected offset once the schema lands).
    let ty = m.schema.field(key).map(|f| f.ty).unwrap_or_else(|| {
        match value.split(',').filter(|s| !s.trim().is_empty()).count() {
            0 | 1 => ParamType::F32,
            2 => ParamType::Vec2,
            3 => ParamType::Vec3,
            _ => ParamType::Vec4,
        }
    });
    match ty {
        ParamType::Vec3 | ParamType::Vec4 => {
            // Colours are authored as `r,g,b` (USD displayColor style).
            let n: Vec<f32> = value.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
            if n.len() >= 3 {
                m.set_color(key, [n[0], n[1], n[2]]);
                true
            } else {
                false
            }
        }
        _ => match ParamValue::parse(ty, value) {
            Some(v) => {
                m.set(key, v);
                true
            }
            None => false,
        },
    }
}

/// Reads the current scalar value for `key` (or its schema default), or
/// `None` if `key` isn't a scalar param.
pub fn get_scalar(m: &ShaderMaterial, key: &str) -> Option<f32> {
    m.get_scalar(key)
}

/// Writes a scalar value by `key`. Returns `false` if `key` isn't in the schema.
pub fn set_scalar_value(m: &mut ShaderMaterial, key: &str, v: f32) -> bool {
    if m.schema.field(key).is_some() {
        m.set_scalar(key, v);
        true
    } else {
        false
    }
}

/// Reads the current colour value for `key` as linear RGB.
pub fn get_color(m: &ShaderMaterial, key: &str) -> Option<[f32; 3]> {
    m.get_color(key)
}

/// Writes a colour value by `key` (linear RGB). Returns `false` if unknown.
pub fn set_color_value(m: &mut ShaderMaterial, key: &str, rgb: [f32; 3]) -> bool {
    if m.schema.field(key).is_some() {
        m.set_color(key, rgb);
        true
    } else {
        false
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Shader reflection — derive each material's parameter schema from its
// shader source (no parameter metadata hardcoded in Rust).
// ─────────────────────────────────────────────────────────────────────────

/// Cache of reflected schemas keyed by shader asset, so each `.wgsl` is parsed
/// once and shared across all materials using it.
#[derive(Resource, Default)]
pub struct ShaderSchemas {
    map: std::collections::HashMap<AssetId<Shader>, Arc<ParamSchema>>,
}

/// The WGSL source of a loaded shader, if it is WGSL.
fn wgsl_source(shader: &Shader) -> Option<&str> {
    match &shader.source {
        ShaderSource::Wgsl(c) => Some(c.as_ref()),
        _ => None,
    }
}

/// Reflects each (re)loaded shader's `Material` struct into a [`ParamSchema`]
/// and assigns it to materials using that shader. Shaders without a `Material`
/// struct reflect to nothing (the material keeps its empty default schema).
pub fn reflect_shader_schemas(
    mut ev: MessageReader<AssetEvent<Shader>>,
    shaders: Res<Assets<Shader>>,
    mut cache: ResMut<ShaderSchemas>,
    mut mats: ResMut<Assets<ShaderMaterial>>,
) {
    for e in ev.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = e {
            if let Some(src) = shaders.get(*id).and_then(wgsl_source) {
                match ParamSchema::parse(src) {
                    Some(s) => {
                        cache.map.insert(*id, Arc::new(s));
                    }
                    None => {
                        cache.map.remove(id);
                    }
                }
            }
        }
    }

    // Assign reflected schemas to materials that don't yet carry them. Find
    // candidates via immutable `iter` (doesn't flag assets dirty), then
    // `get_mut` only the ones that need it (one re-upload each).
    let todo: Vec<(AssetId<ShaderMaterial>, Arc<ParamSchema>)> = mats
        .iter()
        .filter_map(|(id, m)| {
            cache
                .map
                .get(&m.shader.id())
                .filter(|s| !Arc::ptr_eq(s, &m.schema))
                .map(|s| (id, s.clone()))
        })
        .collect();
    for (id, schema) in todo {
        if let Some(m) = mats.get_mut(id) {
            m.set_schema(schema);
        }
    }
}
