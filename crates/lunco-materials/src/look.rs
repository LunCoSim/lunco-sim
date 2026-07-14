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
use std::collections::BTreeMap;

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
}

impl ShaderLook {
    /// A look for `shader` (an asset path) with no parameters set — every value
    /// falls back to the shader's own declared default.
    pub fn new(shader: impl Into<String>) -> Self {
        Self { shader: shader.into(), ..Default::default() }
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

    /// Material-sharing key.
    ///
    /// Floats are quantised (1e-4) so two looks a rounding error apart still share
    /// one handle instead of quietly minting a second material and killing
    /// batching. This is a *sharing* key, not an identity.
    pub fn key(&self) -> ShaderLookKey {
        const Q: f32 = 1.0e4;
        let q = |v: f32| (v * Q).round() as i32;
        let mut values: Vec<(String, Vec<i32>)> = Vec::with_capacity(self.values.len());
        for (name, v) in &self.values {
            let quantised = match v {
                ParamValue::F32(x) => vec![q(*x)],
                ParamValue::Vec2(a) => a.iter().copied().map(q).collect(),
                ParamValue::Vec3(a) => a.iter().copied().map(q).collect(),
                ParamValue::Vec4(a) => a.iter().copied().map(q).collect(),
                // Integers are exact — do NOT quantise them through the float path.
                ParamValue::I32(i) => vec![*i],
                ParamValue::U32(u) => vec![*u as i32],
            };
            values.push((name.clone(), quantised));
        }
        ShaderLookKey {
            shader: self.shader.clone(),
            vertex_shader: self.vertex_shader.clone(),
            values,
            textures: self.textures.iter().map(|(l, h)| (*l, h.id())).collect(),
        }
    }
}

/// Hashable, quantised form of a [`ShaderLook`] — the material-sharing key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ShaderLookKey {
    shader: String,
    vertex_shader: Option<String>,
    values: Vec<(String, Vec<i32>)>,
    textures: Vec<(TextureLayer, AssetId<Image>)>,
}

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

    /// Parameters are an OPEN set — a shader can declare a name Rust has never
    /// heard of, and it round-trips.
    #[test]
    fn parameter_names_are_not_a_closed_set() {
        let look = ShaderLook::new("bespoke.wgsl")
            .with("a_name_rust_has_never_heard_of", ParamValue::F32(1.0));
        assert!(look.values.contains_key("a_name_rust_has_never_heard_of"));
    }
}
