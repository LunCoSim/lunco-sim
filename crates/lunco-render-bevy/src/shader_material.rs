//! General **`ShaderMaterial`** — one material, any WGSL, chosen per-instance.
//!
//! This is the `AsBindGroup` half of the custom-shader boundary, and the reason
//! this crate exists: it is what forces `bevy_pbr` → `bevy_render` → wgpu + naga.
//! The *intent* half — [`ShaderLook`](lunco_materials::ShaderLook), the reflected
//! [`ParamSchema`], the shader catalog, the vertex attribute — stays render-free in
//! `lunco-materials`, so a domain crate can describe a shader look without linking
//! a GPU stack. See `docs/architecture/render-decoupling.md`.
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
//! source (see [`lunco_materials::dyn_params`]). Values are stored by name and packed into
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
//! and optionally the pre-baked horizon shadow cache (filterable `R8Unorm`,
//! sampled with `textureSampleLevel`):
//! ```wgsl
//! @group(#{MATERIAL_BIND_GROUP}) @binding(10) var shadow_cache: texture_2d<f32>;
//! @group(#{MATERIAL_BIND_GROUP}) @binding(11) var shadow_cache_sampler: sampler;
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

use lunco_materials::dyn_params::{self, ParamSchema, ParamType, ParamValue};
use lunco_materials::{
    to_snake_case, ShaderCatalog, ATTRIBUTE_MORPH_NORMAL, ATTRIBUTE_MORPH_TARGET,
};

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
    /// **Layer maps** (terrain layered pipeline, `terrain_layered.wgsl`). All
    /// `Option` + filterable float; `None` binds Bevy's fallback image, and a
    /// shader that doesn't declare the binding is unaffected (same contract as
    /// `height_map`). Sampled by planar UV (`in.uv`), `has_*`-guarded so a
    /// missing map falls back to the procedural look rather than erroring.
    ///
    /// Albedo/colour layer — a real raster (e.g. the NASA lunar colour mosaic
    /// downloaded via `Assets.toml`) blended over the procedural regolith.
    #[texture(2)]
    #[sampler(3)]
    pub albedo_map: Option<Handle<Image>>,
    /// Mineral/classification layer — a class-id or composition raster tinted
    /// through a palette LUT in the shader (also serves science-zone overlays).
    #[texture(4)]
    #[sampler(5)]
    pub mineral_map: Option<Handle<Image>>,
    /// Packed scalar data layers in one RGBA to stay under WebGPU binding
    /// limits: **R=roughness G=ambient-occlusion B=rock-density A=hazard**.
    /// Channel routing lives in the layer stack (`ChannelMap`).
    #[texture(6)]
    #[sampler(7)]
    pub surface_map: Option<Handle<Image>>,
    /// Tangent/world-space normal layer — perturbs the procedural bump normal
    /// (meso-scale relief the FBM can't carry). Typically DEM-derived (Sobel).
    #[texture(8)]
    #[sampler(9)]
    pub normal_map: Option<Handle<Image>>,
    /// **Horizon shadow cache** (terrain shaders, `lunco-environment`'s horizon
    /// system). A filterable `R8Unorm` texture whose texels carry the pre-baked
    /// sun-visibility (0..1) from the SAME ray-march as
    /// `horizon_march.wgsl::sun_visibility`, computed once per sun-direction
    /// change instead of per pixel. The terrain fragment shader does a single
    /// `textureSampleLevel` of this (guarded by the `shadow_cache_on` uniform)
    /// instead of the 48-step march loop. `None` binds Bevy's fallback image;
    /// shaders that don't declare the binding are unaffected (same contract as
    /// `height_map` / the layer maps). Sampled by planar UV (`in.uv`).
    #[texture(10)]
    #[sampler(11)]
    pub shadow_cache: Option<Handle<Image>>,
    /// Per-instance fragment shader. **Not** a bind-group resource — it drives
    /// pipeline specialization (see [`ShaderMaterial::specialize`]) and is kept
    /// as a strong handle so the asset stays loaded.
    pub shader: Handle<Shader>,
    /// Optional per-instance **vertex** shader (e.g. the CDLOD geomorph). Like
    /// [`shader`](Self::shader) it drives pipeline specialization, not the bind
    /// group. When `Some`, [`specialize`](Self::specialize) swaps
    /// `descriptor.vertex.shader` and extends the vertex layout with
    /// [`ATTRIBUTE_MORPH_TARGET`]. `None` (the common case) = Bevy's default mesh
    /// vertex shader, so the fragment-only path is unchanged.
    pub vertex_shader: Option<Handle<Shader>>,
    /// Reflected parameter layout for [`shader`](Self::shader). Defaults to the
    /// legacy fixed layout; the reflect system upgrades it once the shader
    /// source is available. Cheap shared `Arc`. Not a bind-group resource.
    pub schema: Arc<ParamSchema>,
    /// Authored/live parameter values by name; packed into `raw` on change.
    /// Not a bind-group resource.
    pub values: BTreeMap<String, ParamValue>,
    /// Blend mode, surfaced through [`Material::alpha_mode`]. Not a bind-group
    /// resource — it selects the render pipeline.
    ///
    /// `Opaque` by default, which is right for every solid procedural surface
    /// (terrain, panels, struts). An emissive VOLUME needs `Blend`: an exhaust
    /// plume that cannot be seen through is a solid cone.
    pub alpha_mode: AlphaMode,
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
            albedo_map: None,
            mineral_map: None,
            surface_map: None,
            normal_map: None,
            shadow_cache: None,
            shader: Handle::default(),
            vertex_shader: None,
            schema: empty_schema_arc(),
            values: BTreeMap::new(),
            alpha_mode: AlphaMode::Opaque,
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
    /// Inserts/updates a value WITHOUT repacking. Reuses the existing slot when
    /// the key is already present, so the hot re-write path doesn't allocate a
    /// `String` every call (MAT-2). Caller must `repack()` afterwards.
    fn set_value(&mut self, name: &str, v: ParamValue) {
        if let Some(slot) = self.values.get_mut(name) {
            *slot = v;
        } else {
            self.values.insert(name.to_string(), v);
        }
    }
    /// Sets a parameter by name and repacks.
    pub fn set(&mut self, name: &str, v: ParamValue) {
        self.set_value(name, v);
        self.repack();
    }
    /// Applies several named writes and repacks ONCE at the end, rather than a
    /// full 256-byte `repack()` per `set` (MAT-1). Use on multi-param write
    /// paths such as the per-frame engine-field update in the horizon system.
    pub fn set_many<I, S>(&mut self, entries: I)
    where
        I: IntoIterator<Item = (S, ParamValue)>,
        S: AsRef<str>,
    {
        let mut wrote = false;
        for (name, v) in entries {
            self.set_value(name.as_ref(), v);
            wrote = true;
        }
        if wrote {
            self.repack();
        }
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
    pub fn get_vec3(&self, name: &str) -> Option<Vec3> {
        match self.get(name)? {
            ParamValue::Vec3(v) => Some(Vec3::from_array(v)),
            _ => None,
        }
    }
}

/// Pipeline key carrying the per-instance shader handles into `specialize`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ShaderKey {
    shader: Handle<Shader>,
    /// `Some` → swap the vertex stage too + bind the morph-target attribute.
    vertex_shader: Option<Handle<Shader>>,
}

impl From<&ShaderMaterial> for ShaderKey {
    fn from(m: &ShaderMaterial) -> Self {
        Self { shader: m.shader.clone(), vertex_shader: m.vertex_shader.clone() }
    }
}

impl Material for ShaderMaterial {
    /// Bevy folds this into the material's pipeline properties, so a `Blend`
    /// material sorts back-to-front and stops writing depth without anything here
    /// touching the descriptor.
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
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
            // Per-instance VERTEX stage (CDLOD geomorph). Same non-prepass guard:
            // our geomorph `.wgsl` defines only a main-pass `@vertex`, so the
            // prepass keeps Bevy's default vertex shader (un-morphed depth — a
            // sub-tile mismatch acceptable for the debug LOD view). When set, the
            // mesh carries `ATTRIBUTE_MORPH_TARGET` + `ATTRIBUTE_MORPH_NORMAL`, so rebuild
            // the vertex layout to feed `@location(8)` and `@location(9)`. Meshes without it use the fragment-only path
            // above and never reach here (`vertex_shader` is `None`).
            if let Some(vertex) = &key.bind_group_data.vertex_shader {
                descriptor.vertex.shader = vertex.clone();
                let vertex_layout = layout.0.get_layout(&[
                    Mesh::ATTRIBUTE_POSITION.at_shader_location(0),
                    Mesh::ATTRIBUTE_NORMAL.at_shader_location(1),
                    Mesh::ATTRIBUTE_UV_0.at_shader_location(2),
                    ATTRIBUTE_MORPH_TARGET.at_shader_location(8),
                    ATTRIBUTE_MORPH_NORMAL.at_shader_location(9),
                ])?;
                descriptor.vertex.buffers = vec![vertex_layout];
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

/// Keeps the shared `lunco::pbr_lit` WGSL module (PBR-lit mode) loaded so
/// `#import lunco::pbr_lit::lit` resolves in any per-instance shader — letting
/// a self-describing shader opt into bevy's full lighting without hand-copying
/// the PbrInput boilerplate.
#[derive(Resource)]
pub struct PbrLitModule(#[allow(dead_code)] Handle<bevy::shader::Shader>);

/// Keeps the shared `lunco::lunar` WGSL module (lunar regolith photometry —
/// Lommel-Seeliger + opposition surge) loaded so `#import lunco::lunar` resolves
/// in the terrain shaders.
#[derive(Resource)]
pub struct LunarBrdfModule(#[allow(dead_code)] Handle<bevy::shader::Shader>);

/// Keeps the shared `lunco::noise` WGSL module (procedural value noise — the
/// hash/vnoise/fbm family) loaded so `#import lunco::noise` resolves in the
/// terrain and starfield shaders.
#[derive(Resource)]
pub struct NoiseModule(#[allow(dead_code)] Handle<bevy::shader::Shader>);

/// Keeps the shared `lunco::transfer` WGSL module (the value→colour plane of
/// Data → Transfer → Blend) loaded so `#import lunco::transfer` resolves in the
/// terrain shaders. The GPU twin of `lunco_terrain_core::transfer` — one ramp,
/// so an analysis overlay and the legend explaining it cannot disagree.
#[derive(Resource)]
pub struct TransferModule(#[allow(dead_code)] Handle<bevy::shader::Shader>);

impl Plugin for ShaderMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<ShaderMaterial>::default());
        app.init_resource::<ShaderSchemas>();
        app.init_resource::<ShaderCatalog>();
        // Reflect each shader's `Material` struct → per-material `ParamSchema`.
        app.add_systems(Update, reflect_shader_schemas);
        // Catalog discovery lives in ONE place — `lunco-sandbox-edit`'s
        // `maintain_catalogs`, which scans engine + Twin shaders via the shared
        // `lunco_assets::discovery` walk. This crate only seeds the wasm-safe
        // defaults in `ShaderCatalog::default`.
        let module = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/horizon_march.wgsl");
        app.insert_resource(HorizonMarchModule(module));
        let pbr_lit = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/pbr_lit.wgsl");
        app.insert_resource(PbrLitModule(pbr_lit));
        let lunar = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/lunar_brdf.wgsl");
        app.insert_resource(LunarBrdfModule(lunar));
        let noise = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/lunco_noise.wgsl");
        app.insert_resource(NoiseModule(noise));
        let transfer = app
            .world()
            .resource::<AssetServer>()
            .load("shaders/transfer.wgsl");
        app.insert_resource(TransferModule(transfer));
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
///
/// The authored name is normalized to snake_case ([`to_snake_case`]) so
/// USD-conventional camelCase params (`colorA`) land on the snake_case WGSL field
/// (`color_a`). The conversion is idempotent for names that are already
/// snake_case, so the inspector / scripting paths (which use the schema's own
/// field names) are unaffected.
pub fn apply_param(m: &mut ShaderMaterial, key: &str, value: &str) -> bool {
    let key = to_snake_case(key);
    let key = key.as_str();
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
            // Colours are authored as `r,g,b` (USD displayColor style) or
            // `r,g,b,a` for vec4 params.
            let n: Vec<f32> = value.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect();
            if ty == ParamType::Vec4 && n.len() >= 4 {
                m.set(key, ParamValue::Vec4([n[0], n[1], n[2], n[3]]));
                true
            } else if n.len() >= 3 {
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
    mut mat_ev: MessageReader<AssetEvent<ShaderMaterial>>,
    shaders: Res<Assets<Shader>>,
    mut cache: ResMut<ShaderSchemas>,
    mut mats: ResMut<Assets<ShaderMaterial>>,
) {
    let mut cache_changed = false;
    for e in ev.read() {
        if let AssetEvent::Added { id } | AssetEvent::Modified { id } = e {
            if let Some(src) = shaders.get(*id).and_then(wgsl_source) {
                match ParamSchema::parse(src) {
                    Some(s) => {
                        // Every `//!@engine` field must name a registered
                        // provider AND agree with its type — otherwise the fill
                        // writes bytes the shader reinterprets as something
                        // else. Checked once per (re)load, where the reflected
                        // types are known, and warned rather than packed.
                        lunco_materials::engine_params().validate_schema(&s, &format!("{id:?}"));
                        cache.map.insert(*id, Arc::new(s));
                    }
                    None => {
                        cache.map.remove(id);
                    }
                }
                cache_changed = true;
            }
        }
    }

    // The material sweep below is only meaningful when something changed the
    // (material ↔ schema) relationship this frame: a shader event updated the
    // cache, OR a material was added/modified and may now need a schema. With
    // neither, skip the per-frame `mats.iter()` scan entirely (MAT-6). (A
    // freshly added material whose shader is still loading is re-checked when
    // that shader's `Added` event arrives.)
    let mat_changed = mat_ev
        .read()
        .any(|e| matches!(e, AssetEvent::Added { .. } | AssetEvent::Modified { .. }));
    if !cache_changed && !mat_changed {
        return;
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
        if let Some(mut m) = mats.get_mut(id) {
            m.set_schema(schema);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_materials::ParamSchema;
    use std::sync::Arc;

    fn material_with_schema() -> ShaderMaterial {
        let wgsl = "struct Material {\n\
                    base_color: vec3<f32>,\n\
                    color_a: vec3<f32>,\n\
                    morph_start: f32,\n\
                    }";
        let schema = ParamSchema::parse(wgsl).expect("schema parses");
        let mut m = ShaderMaterial::default();
        m.set_schema(Arc::new(schema));
        m
    }

    #[test]
    fn camelcase_authored_params_land_on_snake_fields() {
        let mut m = material_with_schema();
        // USD-conventional camelCase authoring resolves to the snake_case field.
        assert!(apply_param(&mut m, "colorA", "0.1,0.2,0.3"));
        assert!(apply_param(&mut m, "baseColor", "0.4,0.5,0.6"));
        assert!(apply_param(&mut m, "morphStart", "2.0"));

        assert_eq!(m.get_color("color_a"), Some([0.1, 0.2, 0.3]));
        assert_eq!(m.get_color("base_color"), Some([0.4, 0.5, 0.6]));
        assert_eq!(m.get_scalar("morph_start"), Some(2.0));
        // stored under the canonical snake_case key, not the camelCase input
        assert!(m.values.contains_key("color_a"));
        assert!(!m.values.contains_key("colorA"));
    }

    #[test]
    fn snake_case_authoring_still_works() {
        let mut m = material_with_schema();
        assert!(apply_param(&mut m, "color_a", "0.7,0.8,0.9"));
        assert_eq!(m.get_color("color_a"), Some([0.7, 0.8, 0.9]));
    }

    /// A fresh `ShaderMaterial` carries an empty schema and packs all-zero; once
    /// its shader's `Material` struct is reflected, named values pack into the
    /// opaque block at the reflected std140 offsets. (Moved here with the material
    /// itself — `lunco-materials` is render-free and no longer names it.)
    #[test]
    fn shader_material_dynamic_packing() {
        let m = ShaderMaterial::default();
        assert_eq!(m.raw[0].x, 0.0, "empty default packs all-zero");
        assert_eq!(m.raw[5].x, 0.0);

        // Reflect a schema from a shader's `Material` struct and apply params by name.
        let schema = ParamSchema::parse(
            "//!@ui albedo color \"Albedo\"\n\
             struct Material { albedo: vec3<f32>, sun_vis: f32 }\n\
             @group(2) @binding(0) var<uniform> mat: Material;",
        )
        .expect("Material struct reflects");
        let mut m = ShaderMaterial::default();
        m.set_schema(Arc::new(schema));
        m.set("albedo", ParamValue::Vec3([0.4, 0.5, 0.6]));
        m.set_scalar("sun_vis", 1.0);
        // albedo vec3 @ byte 0 → lane 0 .xyz; sun_vis f32 @ byte 12 → lane 0 .w.
        assert_eq!(m.raw[0].x, 0.4);
        assert_eq!(m.raw[0].y, 0.5);
        assert_eq!(m.raw[0].z, 0.6);
        assert_eq!(m.raw[0].w, 1.0);
    }
}
