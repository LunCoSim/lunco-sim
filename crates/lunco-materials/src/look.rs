//! Shader appearance **intent** — a custom WGSL look stated as data, with
//! **user-defined parameters**, without naming a material.
//!
//! # Why this is not `PbrLook`
//!
//! [`lunco_render::PbrLook`] is a *closed* struct: base colour, roughness,
//! metallic. Every field is known to Rust at compile time. That is right for a
//! plain surface and useless for a shader, where **the author decides what the
//! parameters are** — a regolith shader wants `crater_depth` and `dust_scale`; a
//! blueprint shader wants `grid_pitch`. Rust cannot know the set.
//!
//! So [`ShaderLook`] is *open*:
//!
//! - **Parameters are a `BTreeMap<String, ParamValue>`** — any name, any of the
//!   [`ParamType`](crate::ParamType)s. Nothing is hardcoded.
//! - **The names, ranges, defaults and widgets are reflected out of the `.wgsl`
//!   itself** ([`ParamSchema`]), from its `struct Material` block and `//!@`
//!   annotations. Adding a parameter is editing a shader, not editing Rust — and
//!   the Inspector picks it up automatically, because it derives its sliders from
//!   the schema rather than a hand-written list.
//! - The GPU side is a single opaque 256-byte uniform block that **each shader
//!   reinterprets through its own `Material` struct**. That is what makes the set
//!   of parameters a property of the *asset*, not of the engine.
//!
//! # Textures: named layers, and why there are exactly six
//!
//! A "moon look" is several rasters merged by the shader — a colour mosaic, a
//! DEM-derived normal map, a packed scalar layer, a mineral/class map. So texture
//! slots are part of the look, and they are **named**, not positional:
//!
//! ```ignore
//! ShaderLook::new("shaders/terrain_geomorph.wgsl")
//!     .with("dust_scale", ParamValue::F32(0.004))
//!     .with_texture(TextureLayer::Albedo, albedo_handle)
//!     .with_texture(TextureLayer::Normal, dem_normals)
//!     .with_texture(TextureLayer::Surface, packed_rough_ao_rock_hazard)
//! ```
//!
//! # Animated looks must not share
//!
//! The sharing cache is keyed by content, so a look whose value changes **every
//! frame** (a USD `displayColor` timeSample sweep, a pulsing highlight) would mint a
//! fresh material per distinct value and never free the old one — an unbounded leak.
//! Such a look must opt out with [`ShaderLook::unshared`], which gives it a private
//! material the binder mutates in place instead of re-keying.
//!
//! There are six slots and not N because **WebGPU/WebGL2 caps bind-group entries**
//! — arbitrary-N textures needs bindless, which WebGL2 does not have. That is a
//! hardware ceiling, not a design preference. Within it the layers are general: a
//! shader that does not declare a binding simply ignores it (`None` binds Bevy's
//! fallback image), so one slot set serves every shader.
//!
//! # Why this is render-free
//!
//! `Handle<Image>` is `bevy_image`, and `ParamValue` is plain data — neither
//! touches `bevy_pbr`. Only the *binding* of this intent to a real
//! `ShaderMaterial` (an `AsBindGroup`, hence wgpu) does, and that lives in
//! `lunco-render-bevy`. A headless server therefore still holds the full,
//! inspectable, journalable appearance of the scene; it just never turns it into a
//! GPU material.
//!
//! See `docs/architecture/render-decoupling.md`.

use crate::dyn_params::ParamValue;
use bevy::prelude::*;
pub use lunco_render::SurfaceAlpha;
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{DefaultHasher, Hash, Hasher};

/// The named texture layers a shader may sample.
///
/// Fixed set, WebGPU-binding-limited (see the module docs). Shaders opt in by
/// declaring the binding; one that does not is unaffected by a layer being set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Reflect)]
pub enum TextureLayer {
    /// R32Float world heights — ray-marched sun shadows. Non-filterable.
    Height,
    /// Colour raster (e.g. the NASA lunar mosaic) blended over the procedural look.
    Albedo,
    /// Class-id / composition raster, tinted through a palette LUT in the shader.
    Mineral,
    /// Packed scalars in one RGBA to stay under the binding cap:
    /// **R = roughness, G = ambient occlusion, B = rock density, A = hazard.**
    Surface,
    /// Tangent/world-space normals — DEM-derived relief the procedural FBM cannot carry.
    Normal,
    /// Pre-baked sun visibility (R8Unorm), so the fragment shader samples once
    /// instead of running the 48-step horizon march.
    ShadowCache,
}

/// A custom-shader surface, stated as data.
///
/// Insert next to `Mesh3d`; `lunco-render-bevy` binds it to a real `ShaderMaterial`.
///
/// **Identical looks share one material.** The binder caches by [`ShaderLook::key`],
/// so N tiles in the same LOD band and reveal step cost one material and one bind
/// group — the batching property the terrain LOD path depends on. Vary a param
/// per-instance and you mint a material per instance; bucket it instead.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct ShaderLook {
    /// Fragment shader asset path, e.g. `"shaders/terrain_geomorph.wgsl"`.
    ///
    /// A path, not a `Handle<Shader>`, on purpose: `bevy::shader::Shader` lives in
    /// `bevy_shader`, which pulls **naga**. The binder loads the handle.
    pub shader: String,
    /// Optional vertex shader (e.g. the CDLOD geomorph). `None` = Bevy's default.
    pub vertex_shader: Option<String>,
    /// **The open set.** Parameter name → value. Names come from the shader's own
    /// `struct Material`; Rust hardcodes none of them.
    pub values: BTreeMap<String, ParamValue>,
    /// Params that are **not part of material identity** — excluded from
    /// [`key`](Self::key), so changing one re-uses the same material and the binder
    /// writes the new value into it in place.
    ///
    /// For a param that is *globally uniform* and *continuously tuned*: the
    /// slope-hazard overlay's angles, dragged on a slider. Putting such a value in
    /// [`values`](Self::values) is correct but mints a fresh material per distinct
    /// value — every tile then rebinds to a handle whose bind group is not prepared
    /// yet, which reads as the terrain flickering for the length of the drag. (The
    /// old materials also survive until the cache sweep.) `unshared` would fix the
    /// churn but hands each of ~500 tiles a private material, which is the
    /// draw-call blow-up the sharing cache exists to prevent. This is the third
    /// option: one shared material, mutated.
    ///
    /// **The invariant:** two looks that differ ONLY here share a material, so the
    /// last writer wins. Only put a value here when every look that could share
    /// this material carries the same one — i.e. it is driven by a single global
    /// resource. A per-entity value belongs in `values`.
    pub live: BTreeMap<String, ParamValue>,
    /// Parameter names this prim's USD authoring drives through a connection.
    ///
    /// Authored by the USD shader pass, which resolves the bound shader and so knows
    /// exactly which of the prim's `inputs:*.connect` name parameters that shader
    /// declares. The port backend accepts a write if and only if the name is here.
    /// `inputs:` is the engine's spelling for every port, so this set is what
    /// separates a shader drive from a simulation wire on the same prim — a value
    /// the authoring layer knows and the render layer cannot infer.
    ///
    /// Not part of [`key`](Self::key): it says where values come from, not what the
    /// material looks like.
    pub driven: BTreeSet<String>,
    /// Named texture layers. Absent = the shader's fallback.
    pub textures: BTreeMap<TextureLayer, Handle<Image>>,
    /// Opt out of material sharing — this look gets a **private** material that the
    /// binder mutates in place.
    ///
    /// Set this for anything that changes every frame (an animated `displayColor`, a
    /// pulsing highlight). Otherwise the content-keyed cache mints a fresh material
    /// per distinct value and never frees the previous one — an unbounded leak that
    /// looks like a slow memory climb, not a bug.
    ///
    /// The cost is a material and a bind group of your own: correct for the handful
    /// of animated prims, ruinous if you set it on 6000 rocks.
    pub unshared: bool,
    /// This mesh must not be rasterised into the sun's shadow map
    /// (`primvars:doNotCastShadows` — Omniverse's name, the same one
    /// [`lunco_render::PbrLook`] reads).
    ///
    /// [`PbrLook`](lunco_render::PbrLook) already carried this and the shader path
    /// did not, which is a real asymmetry: taking the shader path *removes* the
    /// `PbrLook`, so a prim that authored the primvar silently started casting the
    /// moment it was given a `.wgsl`.
    ///
    /// It is load-bearing for anything that ENCLOSES the scene. A sky dome is the
    /// worked example: a shell of radius R sits between the sun and every cascade,
    /// so the shadow pass sees a solid occluder covering the whole frustum and the
    /// entire scene renders in shadow. The mesh is emissive and infinitely distant
    /// in intent — it has no business in a shadow map at all.
    ///
    /// **Not part of [`key`](Self::key).** It is an entity-level render flag, not
    /// material state, so two looks that differ only here still share one material
    /// and one bind group.
    pub no_shadow_cast: bool,
    /// How this surface handles transparency — the same [`SurfaceAlpha`] a
    /// [`PbrLook`](lunco_render::PbrLook) carries, from the same authored
    /// `primvars:displayOpacity`.
    ///
    /// Here for the reason `no_shadow_cast` is: taking the shader path REMOVES the
    /// `PbrLook`, so without this a translucent prim turns opaque the moment it is
    /// given a `.wgsl`. An emissive VOLUME — an exhaust plume, a beam — is the case
    /// that cannot work at all without it, because what shows through it is what it
    /// is.
    ///
    /// **Part of [`key`](Self::key), unlike the shadow flag.** Blend mode selects
    /// the render pipeline the material binds, so two looks that differ here cannot
    /// share one material.
    pub alpha: SurfaceAlpha,
    /// Render both faces (authored `doubleSided` on the gprim — the standard
    /// `UsdGeomGprim` attribute, the same one the PBR path maps to
    /// `cull_mode: None`).
    ///
    /// Here for the reason `no_shadow_cast` and `alpha` are: taking the shader
    /// path REMOVES the `PbrLook` that carried it, so a `doubleSided` prim
    /// silently became backface-culled the moment it was given a `.wgsl`. The
    /// sky dome is the worked example: viewed from INSIDE, only its back faces
    /// are visible, so dropping the flag culls the entire sky.
    ///
    /// **Part of [`key`](Self::key), like `alpha`.** Cull mode is pipeline
    /// state, so two looks that differ here cannot share one material.
    pub double_sided: bool,
}

impl ShaderLook {
    /// A look for `shader` (an asset path) with no parameters set — every value
    /// falls back to the shader's own declared default.
    pub fn new(shader: impl Into<String>) -> Self {
        Self {
            shader: shader.into(),
            ..Default::default()
        }
    }

    /// Set one parameter. The name must exist in the shader's `struct Material`;
    /// an unknown name is dropped at pack time (with a warning), never silently
    /// mis-packed into a neighbouring field.
    pub fn with(mut self, name: impl Into<String>, value: ParamValue) -> Self {
        self.values.insert(name.into(), value);
        self
    }

    /// Bind a texture layer.
    pub fn with_texture(mut self, layer: TextureLayer, image: Handle<Image>) -> Self {
        self.textures.insert(layer, image);
        self
    }

    /// Use `vertex` as the vertex shader (asset path).
    pub fn with_vertex_shader(mut self, vertex: impl Into<String>) -> Self {
        self.vertex_shader = Some(vertex.into());
        self
    }

    /// Give this look a **private** material instead of a shared one — required for
    /// anything animated. See [`ShaderLook::unshared`](Self::unshared).
    pub fn unshared(mut self) -> Self {
        self.unshared = true;
        self
    }

    /// Set one **live** param — outside the sharing key, written into the shared
    /// material in place. See [`live`](Self::live) for when this is legitimate.
    pub fn set_live(&mut self, name: impl Into<String>, value: ParamValue) {
        self.live.insert(name.into(), value);
    }

    /// Material-sharing key.
    ///
    /// Floats are quantised (1e-4) so two looks a rounding error apart still share
    /// one handle instead of quietly minting a second material and killing
    /// batching. This is a *sharing* key, not an identity.
    ///
    /// Computed by feeding a `Hasher` directly — no `String` clones, no `Vec`s.
    /// The old struct-of-clones key allocated a `String` per param per call, and
    /// the terrain path calls this per tile per `Changed<ShaderLook>`, so key
    /// construction itself showed up in the frame. A 64-bit content hash makes
    /// the key `Copy`; the collision odds over the few hundred distinct looks a
    /// scene holds are negligible for a SHARING key (a collision would merely
    /// serve one look the other's material — vanishingly unlikely, and this was
    /// never a correctness identity).
    pub fn key(&self) -> ShaderLookKey {
        const Q: f32 = 1.0e4;
        let q = |v: f32| (v * Q).round() as i32;
        let mut h = DefaultHasher::new();
        self.shader.hash(&mut h);
        self.vertex_shader.hash(&mut h);
        for (name, v) in &self.values {
            name.hash(&mut h);
            match v {
                ParamValue::F32(x) => q(*x).hash(&mut h),
                ParamValue::Vec2(a) => a.iter().for_each(|x| q(*x).hash(&mut h)),
                ParamValue::Vec3(a) => a.iter().for_each(|x| q(*x).hash(&mut h)),
                ParamValue::Vec4(a) => a.iter().for_each(|x| q(*x).hash(&mut h)),
                // Integers are exact — do NOT quantise them through the float path.
                // Discriminant-tagged so `I32(5)` and `U32(5)` don't collide with
                // a quantised float by lane content alone.
                ParamValue::I32(i) => (0x49u8, *i).hash(&mut h),
                ParamValue::U32(u) => (0x55u8, *u).hash(&mut h),
            }
        }
        for (layer, image) in &self.textures {
            (*layer, image.id()).hash(&mut h);
        }
        // `SurfaceAlpha` carries an f32 in one arm, so it is `PartialEq` and not
        // `Hash`/`Eq`. Quantise the threshold exactly as the values above are
        // quantised — a mask cutoff a rounding error apart is the same pipeline.
        match self.alpha {
            SurfaceAlpha::Opaque => (0u8, 0i32),
            SurfaceAlpha::Mask(t) => (1, q(t)),
            SurfaceAlpha::Blend => (2, 0),
            SurfaceAlpha::Add => (3, 0),
        }
        .hash(&mut h);
        self.double_sided.hash(&mut h);
        ShaderLookKey(h.finish())
    }
}

/// Quantised 64-bit content hash of a [`ShaderLook`] — the material-sharing key.
/// See [`ShaderLook::key`] for what is hashed and why a hash suffices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShaderLookKey(u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_looks_share_a_key() {
        let a = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.5));
        let b = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.5));
        assert_eq!(a.key(), b.key());
    }

    /// The point of quantising: a float a hair apart must NOT mint a second
    /// material. If this regresses, batching dies silently.
    #[test]
    fn a_rounding_error_apart_still_shares() {
        let a = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.5));
        let b = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.5 + 1e-7));
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn a_real_difference_does_not_share() {
        let a = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.5));
        let b = ShaderLook::new("s.wgsl").with("dust", ParamValue::F32(0.7));
        assert_ne!(a.key(), b.key());
        let c = ShaderLook::new("other.wgsl").with("dust", ParamValue::F32(0.5));
        assert_ne!(a.key(), c.key());
    }

    /// Blend mode selects the render PIPELINE, so two otherwise-identical looks
    /// that disagree about transparency must not collapse onto one material — an
    /// opaque plume is not a dimmer plume, it is a solid cone.
    #[test]
    fn alpha_mode_is_part_of_material_identity() {
        let opaque = ShaderLook::new("plume.wgsl");
        let blended = ShaderLook {
            alpha: SurfaceAlpha::Blend,
            ..ShaderLook::new("plume.wgsl")
        };
        assert_ne!(opaque.key(), blended.key());
    }

    /// Parameters are an OPEN set — a shader can declare a name Rust has never
    /// heard of, and it round-trips.
    #[test]
    fn parameter_names_are_not_a_closed_set() {
        let look = ShaderLook::new("bespoke.wgsl")
            .with("a_name_rust_has_never_heard_of", ParamValue::F32(1.0));
        assert!(look.values.contains_key("a_name_rust_has_never_heard_of"));
    }
}
