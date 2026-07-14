//! Appearance **intent** — what a thing should look like, stated without naming a
//! material.
//!
//! # Why this exists
//!
//! `MeshMaterial3d<M>` and `StandardMaterial` live in `bevy_pbr`, and `bevy_pbr`
//! pulls `bevy_render` → wgpu + naga. So *any* crate that binds a material — even
//! one whose material code is already skipped at runtime headless — drags the
//! whole GPU stack into every build that links it, including the `--no-ui` server
//! and the wasm worker.
//!
//! The Bevy 0.19 crate split is kinder than it looks. These are all render-FREE:
//!
//! | free | forces wgpu |
//! |---|---|
//! | `bevy_mesh` — `Mesh`, **`Mesh3d`** | `bevy_pbr` — `MeshMaterial3d`, `StandardMaterial` |
//! | `bevy_camera` — `Camera`, `Visibility`, `VisibilityRange` | `bevy_core_pipeline` — `Camera3d`, `Bloom` |
//! | `bevy_light`, `bevy_image`, `bevy_asset`, `bevy_gltf` | `bevy_shader` (naga) |
//!
//! Geometry, transforms, lights, cameras and visibility can all exist headless.
//! **The material is the boundary — and it is the only one.**
//!
//! # The rule
//!
//! > A domain crate may name `Mesh3d`. It may not name `MeshMaterial3d`.
//!
//! Domain crates spawn geometry plus an intent component from this module and
//! stop. `lunco-render-bevy` — the single crate in the workspace that depends on
//! `bevy_pbr` — observes the intent and binds the concrete material. Headless
//! simply never adds that plugin, so there is no `#[cfg]` anywhere in the
//! simulation. The gate is *which plugins you add*, not conditional compilation.
//!
//! A pleasant side effect: appearance becomes ECS data rather than an opaque
//! `Handle<StandardMaterial>` — so it is inspectable, serializable to USD, and
//! replicable over the wire, none of which a handle can be.
//!
//! See `docs/architecture/render-decoupling.md`.

use bevy::prelude::*;

/// How a surface handles transparency. Mirrors `bevy::pbr::AlphaMode` without
/// naming it (that type is in `bevy_pbr`, which is the whole thing we are avoiding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub enum SurfaceAlpha {
    #[default]
    Opaque,
    /// Cut out fragments below `threshold` — foliage, decals, filename labels.
    Mask(f32),
    /// Sorted alpha blending.
    Blend,
    /// Additive. Distinct from `Blend` only where the destination is NOT black —
    /// which for an orbit line means exactly where it crosses a lit body. Over the
    /// sky the two are identical (`dst + src·a` ≡ `dst·(1−a) + src·a` when `dst ≈ 0`).
    Add,
}

/// The texture channels a PBR surface can carry.
///
/// `Handle<Image>` is `bevy_image` — render-free — so a texture-bearing surface is
/// still expressible without `bevy_pbr`. These map 1:1 onto `StandardMaterial`'s
/// channels and cover what UsdPreviewSurface actually authors.
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
pub struct PbrTextures {
    pub base_color: Option<Handle<Image>>,
    pub emissive: Option<Handle<Image>>,
    pub metallic_roughness: Option<Handle<Image>>,
    pub normal_map: Option<Handle<Image>>,
    pub occlusion: Option<Handle<Image>>,
}

/// A PBR surface, stated as data.
///
/// Insert alongside `Mesh3d`; `lunco-render-bevy` turns it into
/// `MeshMaterial3d<StandardMaterial>`.
///
/// **Identical `PbrLook`s share one material handle.** The binder caches by
/// [`PbrLook::key`], so scattering 6000 rocks with the same look costs one
/// material and one bind group, not 6000 — the batching property several of the
/// terrain/rock perf fixes depend on. Do not defeat it by varying a field
/// per-instance; if instances must differ, bucket the values first (see
/// `lunco-terrain-surface`'s rock buckets for the pattern).
///
/// **Anything ANIMATED must set [`unshared`](Self::unshared)** — otherwise a value
/// that changes every frame re-keys the cache every frame, minting a material per
/// distinct value and freeing none. That is an unbounded leak which presents as a
/// slow memory climb rather than an obvious bug.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct PbrLook {
    /// Linear base colour.
    pub base_color: LinearRgba,
    /// 0 = mirror, 1 = fully rough. Lunar regolith is ~1.0.
    pub perceptual_roughness: f32,
    /// 0 = dielectric, 1 = metal.
    pub metallic: f32,
    /// Skip lighting entirely: output `base_color` verbatim, ignoring lights, normals
    /// and shadows.
    ///
    /// **Render intent, NOT a material property — and deliberately not persisted.**
    /// It says "this geometry is a *symbol*, not a surface": trajectory lines, the
    /// terrain brush overlay, name labels. Asking how the sun falls on an orbit line
    /// is a category error, which is why `UsdPreviewSurface` has no such input and is
    /// right not to.
    ///
    /// It matters more here than in a terrestrial renderer: the Moon has no
    /// atmosphere, so there is no ambient fill, and geometry facing away from the sun
    /// renders *pure black*. A lit trajectory line would vanish on the night side —
    /// exactly where you need to see where the spacecraft is going.
    ///
    /// Set from Rust at the few overlay call sites. No `.usda` authors it. If a
    /// *scene* surface ever needs to be unlit, say so the USD way — an emissive-only
    /// `UsdPreviewSurface` (`diffuseColor` 0, `emissiveColor` C, `specularColor` 0) —
    /// rather than reaching for this flag.
    pub unlit: bool,
    /// Render back faces too.
    pub double_sided: bool,
    /// Do not cast shadows. Terrain tiles and scattered rocks set this — it is a
    /// large, measured saving, not a cosmetic choice.
    pub no_shadow_cast: bool,
    /// Emissive radiance.
    pub emissive: LinearRgba,
    /// Transparency handling.
    pub alpha: SurfaceAlpha,
    /// Texture channels (UsdPreviewSurface authors all five).
    pub textures: PbrTextures,
    /// Index of refraction — `UsdPreviewSurface`'s `inputs:ior`, default 1.5 (glass;
    /// silicates sit at 1.5–1.6).
    ///
    /// This is the ONLY specular-strength knob, deliberately. IOR is the physical
    /// cause; the normal-incidence reflectance F₀ is its consequence, via Fresnel:
    /// `F0 = ((1 - ior) / (1 + ior))²` — so `ior` 1.5 is the familiar 4% dielectric.
    ///
    /// There used to be a second field, `reflectance`, carrying Bevy/Filament's
    /// artist remap of that SAME quantity (`F0 = 0.16 · reflectance²`, where 0.5 also
    /// means 4%). Two fields for one physical fact let a look claim to reflect like
    /// diamond and refract like glass — a substance that does not exist, which the
    /// renderer drew anyway. It also had nowhere to persist: USD stores `ior` and has
    /// no `reflectance`, so the value was smuggled into a private `inputs:reflectance`
    /// that only this codebase could read. Filament's curve is a fact about *Bevy*,
    /// not about the material, so it now lives in `lunco-render-bevy` alone.
    pub ior: f32,
    /// Clearcoat layer strength (0 = none).
    pub clearcoat: f32,
    /// Roughness of the clearcoat layer.
    pub clearcoat_perceptual_roughness: f32,
    /// Specular tint. UsdPreviewSurface authors this as `inputs:specularColor` under
    /// `useSpecularWorkflow = 1`; without it such a prim renders with an untinted
    /// (white) specular highlight.
    pub specular_tint: LinearRgba,
    /// Opt out of material sharing — a **private** material the binder mutates in
    /// place. **Required for animated looks** (see the type docs). Costs one material
    /// and one bind group: correct for a handful of animated prims, ruinous on 6000
    /// rocks.
    pub unshared: bool,
}

impl Default for PbrLook {
    fn default() -> Self {
        Self {
            base_color: LinearRgba::rgb(0.5, 0.5, 0.5),
            perceptual_roughness: 1.0,
            metallic: 0.0,
            unlit: false,
            double_sided: false,
            no_shadow_cast: false,
            emissive: LinearRgba::BLACK,
            alpha: SurfaceAlpha::Opaque,
            textures: PbrTextures::default(),
            // `StandardMaterial`'s own defaults — do not drift from them. `ior` 1.5 is
            // also `UsdPreviewSurface`'s default, and maps to Bevy's `reflectance` 0.5.
            ior: 1.5,
            clearcoat: 0.0,
            clearcoat_perceptual_roughness: 0.0,
            specular_tint: LinearRgba::WHITE,
            unshared: false,
        }
    }
}

impl PbrLook {
    /// An opaque, matte surface of `color` — the common case.
    pub fn matte(color: LinearRgba) -> Self {
        Self { base_color: color, ..Default::default() }
    }

    /// Builder: this look casts no shadows.
    pub fn no_shadows(mut self) -> Self {
        self.no_shadow_cast = true;
        self
    }

    /// Builder: give this look a **private** material instead of a shared one —
    /// required for anything animated. See [`unshared`](Self::unshared).
    pub fn unshared(mut self) -> Self {
        self.unshared = true;
        self
    }

    /// Cache key for material sharing.
    ///
    /// Floats are quantised (1e-4) before hashing so that two looks a rounding
    /// error apart still share a handle rather than minting a second material and
    /// silently breaking batching. That is the whole point of the key: it is a
    /// *sharing* key, not an identity.
    ///
    /// Textures participate by `AssetId`, so two looks that differ only in their
    /// albedo correctly get two materials — which is exactly how a near terrain tile
    /// carries a 2048² raster while a far one carries a 256².
    pub fn key(&self) -> PbrLookKey {
        const Q: f32 = 1.0e4;
        let q = |v: f32| (v * Q).round() as i32;
        let rgba = |c: LinearRgba| [q(c.red), q(c.green), q(c.blue), q(c.alpha)];
        let tex = |h: &Option<Handle<Image>>| h.as_ref().map(|h| h.id());
        PbrLookKey {
            base_color: rgba(self.base_color),
            emissive: rgba(self.emissive),
            perceptual_roughness: q(self.perceptual_roughness),
            metallic: q(self.metallic),
            ior: q(self.ior),
            clearcoat: q(self.clearcoat),
            clearcoat_perceptual_roughness: q(self.clearcoat_perceptual_roughness),
            specular_tint: rgba(self.specular_tint),
            alpha: match self.alpha {
                SurfaceAlpha::Opaque => (0, 0),
                SurfaceAlpha::Mask(t) => (1, q(t)),
                SurfaceAlpha::Blend => (2, 0),
                SurfaceAlpha::Add => (3, 0),
            },
            textures: [
                tex(&self.textures.base_color),
                tex(&self.textures.emissive),
                tex(&self.textures.metallic_roughness),
                tex(&self.textures.normal_map),
                tex(&self.textures.occlusion),
            ],
            flags: (self.unlit as u8)
                | (self.double_sided as u8) << 1
                | (self.no_shadow_cast as u8) << 2,
        }
    }
}

/// Hashable, quantised form of a [`PbrLook`] — the material-sharing key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PbrLookKey {
    base_color: [i32; 4],
    emissive: [i32; 4],
    perceptual_roughness: i32,
    metallic: i32,
    ior: i32,
    clearcoat: i32,
    clearcoat_perceptual_roughness: i32,
    specular_tint: [i32; 4],
    /// `(discriminant, quantised threshold)`.
    alpha: (u8, i32),
    textures: [Option<AssetId<Image>>; 5],
    flags: u8,
}
