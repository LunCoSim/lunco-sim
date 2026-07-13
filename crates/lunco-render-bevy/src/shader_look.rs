//! The `ShaderLook` → `ShaderMaterial` binder — the custom-shader half of the
//! render boundary.
//!
//! [`lunco_render::PbrLook`] covers a plain PBR surface; a *custom shader* look is
//! open-ended (the parameter set belongs to the `.wgsl`, not to Rust), so domain
//! crates state it as [`lunco_materials::ShaderLook`] — a shader **path**, a
//! `BTreeMap` of named [`ParamValue`](lunco_materials::ParamValue)s, and named
//! [`TextureLayer`]s. Neither the path nor `Handle<Image>` touches `bevy_pbr`, so
//! the crate that authors the look (the terrain streamer, notably) links no GPU
//! stack. This module is where it becomes a real `ShaderMaterial`.
//!
//! # The cache is load-bearing
//!
//! [`ShaderLookCache`] maps [`ShaderLookKey`] → one `Handle<ShaderMaterial>`. The
//! terrain LOD path depends on it: the ~150–500 resident tiles collapse onto a
//! handful of distinct looks (mode × morph-band bucket × reveal step), and they
//! MUST resolve to the same material — one bind group, one batch. This is exactly
//! the hand-rolled `LodMaterials`/`MatKey` cache the terrain used to carry, done
//! once, generically, keyed by the look's own content.
//!
//! Nothing here mutates a material after it is built: a tile that changes (a
//! reveal step, an overlay re-tune, a late-bound derived map) edits its
//! `ShaderLook`, and the binder swaps the *handle* to another cached material. No
//! per-frame `Assets::get_mut`, so no uniform re-upload and no `AssetEvent`
//! storm — the property `R5` bought and this must not give back.
//!
//! The schema (parameter name → std140 offset, reflected out of the WGSL) is
//! filled in by [`reflect_shader_schemas`](crate::reflect_shader_schemas) once the shader
//! source loads; a freshly built material carries the empty schema and its values
//! by name, and is repacked the moment the schema lands. That machinery is
//! untouched.

use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::shader::Shader;
use crate::look_cache::{sweep_look_cache, CachedLook, LookCache};
use crate::shader_material::{build_shader_material, ShaderMaterial};
use lunco_materials::{ShaderLook, ShaderLookKey, TextureLayer};

/// Shared `ShaderMaterial` per distinct [`ShaderLookKey`] — see the module docs.
/// Sharing, the `unshared` bypass, and eviction all live in
/// [`LookCache`](crate::look_cache::LookCache), shared with the PBR binder.
pub type ShaderLookCache = LookCache<ShaderLook>;

impl CachedLook for ShaderLook {
    type Key = ShaderLookKey;
    type Material = ShaderMaterial;

    fn look_key(&self) -> ShaderLookKey {
        self.key()
    }
    fn is_unshared(&self) -> bool {
        self.unshared
    }
}

/// Build the concrete `ShaderMaterial` a look describes.
fn shader_material(look: &ShaderLook, asset_server: &AssetServer) -> ShaderMaterial {
    let mut m = ShaderMaterial {
        // A path, not a handle, in the intent — `bevy::shader` pulls naga, so the
        // domain crate cannot hold `Handle<Shader>`. Load it here.
        vertex_shader: look
            .vertex_shader
            .clone()
            .map(|p| asset_server.load::<Shader>(p)),
        values: look.values.clone(),
        ..Default::default()
    };
    for (layer, image) in &look.textures {
        let slot = match layer {
            TextureLayer::Height => &mut m.height_map,
            TextureLayer::Albedo => &mut m.albedo_map,
            TextureLayer::Mineral => &mut m.mineral_map,
            TextureLayer::Surface => &mut m.surface_map,
            TextureLayer::Normal => &mut m.normal_map,
            TextureLayer::ShadowCache => &mut m.shadow_cache,
        };
        *slot = Some(image.clone());
    }
    // Packs against the (initially empty) schema; `reflect_shader_schemas` upgrades
    // it and repacks once the WGSL source lands. Same lifecycle as every other
    // `ShaderMaterial` in the codebase.
    m.repack();
    build_shader_material(asset_server.load::<Shader>(look.shader.clone()), m)
}

/// Resolve a look to a handle. Sharing + the `unshared` bypass are
/// [`LookCache::resolve`]'s job; this only supplies the build recipe.
fn material_for(
    look: &ShaderLook,
    cache: &mut ShaderLookCache,
    materials: &mut Assets<ShaderMaterial>,
    asset_server: &AssetServer,
) -> Handle<ShaderMaterial> {
    cache.resolve(look, materials, |l| shader_material(l, asset_server))
}

/// `On<Add, ShaderLook>` — the moment intent appears, give it a material.
fn bind_shader_look(
    add: On<Add, ShaderLook>,
    looks: Query<&ShaderLook>,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    let e = add.entity;
    let Ok(look) = looks.get(e) else { return };
    let handle = material_for(look, &mut cache, &mut materials, &asset_server);
    commands.entity(e).try_insert(MeshMaterial3d(handle));
}

/// Re-bind when a look is edited in place — a terrain tile crossing a reveal step,
/// an overlay re-tune, a late-bound derived map, an Inspector edit.
///
/// Change-driven, and it swaps a *handle* from the cache; it never touches the
/// material asset. A static scene costs nothing.
fn rebind_changed_shader_look(
    changed: Query<
        (Entity, &ShaderLook, Option<&MeshMaterial3d<ShaderMaterial>>),
        Changed<ShaderLook>,
    >,
    mut cache: ResMut<ShaderLookCache>,
    mut materials: ResMut<Assets<ShaderMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (e, look, current) in &changed {
        if look.unshared {
            // Private material: overwrite the asset it already owns, rather than
            // adding one per change (that would leak a material per frame).
            if let Some(mut existing) = current.and_then(|m| materials.get_mut(&m.0)) {
                *existing = shader_material(look, &asset_server);
                continue;
            }
        }
        let handle = material_for(look, &mut cache, &mut materials, &asset_server);
        commands.entity(e).try_insert(MeshMaterial3d(handle));
    }
}

/// Wire the `ShaderLook` binder into an app. Called by
/// [`LuncoRenderPlugin`](crate::LuncoRenderPlugin).
///
/// NOTE: this does **not** add [`ShaderMaterialPlugin`](crate::ShaderMaterialPlugin)
/// — [`LuncoRenderPlugin`](crate::LuncoRenderPlugin) does, right after calling this,
/// and exactly once (the hand-rolled adds in `lunco-sandbox` and `luncosim` were
/// deleted; Bevy panics on a duplicate plugin). Keeping the two separate lets this
/// binder be unit-tested on a bare `MinimalPlugins` app, with no render pipeline.
pub(crate) fn build(app: &mut App) {
    // The `ShaderMaterial` store must exist for the binder even before the
    // pipeline plugin registers it (plugin order is not ours to control), and the
    // `Shader` asset must be registered for `asset_server.load::<Shader>` not to
    // panic. Both are idempotent — `MaterialPlugin`'s own `init_asset` is a no-op
    // when the store already exists, so nothing is double-registered.
    bevy::asset::AssetApp::init_asset::<ShaderMaterial>(app);
    bevy::asset::AssetApp::init_asset::<Shader>(app);
    app.init_resource::<ShaderLookCache>()
        .add_observer(bind_shader_look)
        .add_systems(
            Update,
            (rebind_changed_shader_look, sweep_look_cache::<ShaderLook>),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_materials::ParamValue;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        // `Image` is registered by `ImagePlugin` in a real build; the texture-layer
        // test needs it in this bare one.
        app.init_asset::<Image>();
        build(&mut app);
        app
    }

    fn material_of(app: &App, e: Entity) -> Handle<ShaderMaterial> {
        app.world()
            .entity(e)
            .get::<MeshMaterial3d<ShaderMaterial>>()
            .expect("bound")
            .0
            .clone()
    }

    /// THE property the cache exists for: N tiles in the same LOD band + reveal
    /// step must share ONE material and ONE bind group. If this regresses, terrain
    /// batching dies and the draw-call count goes linear in the tile count.
    #[test]
    fn identical_looks_share_one_material() {
        let mut app = app();
        let look = ShaderLook::new("shaders/terrain_geomorph.wgsl")
            .with_vertex_shader("shaders/terrain_geomorph.wgsl")
            .with("morph_start", ParamValue::F32(0.7))
            .with("morph_end", ParamValue::F32(1.0));
        let ids: Vec<Entity> = (0..64).map(|_| app.world_mut().spawn(look.clone()).id()).collect();
        app.update();

        let handles: Vec<_> = ids.iter().map(|&e| material_of(&app, e)).collect();
        assert!(
            handles.windows(2).all(|w| w[0] == w[1]),
            "64 identical looks must share one material handle"
        );
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 1);
        assert_eq!(app.world().resource::<ShaderLookCache>().len(), 1);
    }

    /// Two genuinely different looks must NOT collide into one material.
    #[test]
    fn different_looks_get_different_materials() {
        let mut app = app();
        app.world_mut().spawn(
            ShaderLook::new("shaders/terrain_geomorph.wgsl").with("reveal", ParamValue::F32(0.0)),
        );
        app.world_mut().spawn(
            ShaderLook::new("shaders/terrain_geomorph.wgsl").with("reveal", ParamValue::F32(1.0)),
        );
        // A different shader path is also a different material.
        app.world_mut().spawn(ShaderLook::new("shaders/terrain_geomorph_flat.wgsl"));
        app.update();
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 3);
    }

    /// A `Changed<ShaderLook>` re-binds — this is how a tile's reveal step and the
    /// live overlay re-tune reach the GPU, WITHOUT mutating any material asset.
    #[test]
    fn changed_look_rebinds_from_the_cache() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn(ShaderLook::new("shaders/terrain_geomorph.wgsl").with("reveal", ParamValue::F32(0.0)))
            .id();
        app.update();
        let first = material_of(&app, e);

        // Step the reveal — the same edit `animate_tile_reveal` makes.
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ShaderLook>()
            .unwrap()
            .values
            .insert("reveal".into(), ParamValue::F32(0.5));
        app.update();
        let second = material_of(&app, e);
        assert_ne!(first, second, "a changed look must bind a different material");
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 2);

        // …and stepping BACK to a look already seen reuses the cached material
        // instead of minting a third (the reveal lattice is a small shared set).
        app.world_mut()
            .entity_mut(e)
            .get_mut::<ShaderLook>()
            .unwrap()
            .values
            .insert("reveal".into(), ParamValue::F32(0.0));
        app.update();
        assert_eq!(material_of(&app, e), first);
        assert_eq!(app.world().resource::<Assets<ShaderMaterial>>().len(), 2);
    }

    /// Texture layers land on the right `ShaderMaterial` slots, and two looks that
    /// differ ONLY by a bound texture do not share a material (per-place quality:
    /// the near tile's 2048² albedo and the far tile's 256² one are two materials).
    #[test]
    fn texture_layers_map_onto_material_slots() {
        let mut app = app();
        let surface: Handle<Image> = app.world().resource::<AssetServer>().load("a.png");
        let normal: Handle<Image> = app.world().resource::<AssetServer>().load("b.png");
        let e = app
            .world_mut()
            .spawn(
                ShaderLook::new("shaders/terrain_geomorph.wgsl")
                    .with_texture(TextureLayer::Surface, surface.clone())
                    .with_texture(TextureLayer::Normal, normal.clone()),
            )
            .id();
        app.world_mut().spawn(ShaderLook::new("shaders/terrain_geomorph.wgsl"));
        app.update();

        let h = material_of(&app, e);
        let mats = app.world().resource::<Assets<ShaderMaterial>>();
        let m = mats.get(&h).expect("material");
        assert_eq!(m.surface_map.as_ref(), Some(&surface));
        assert_eq!(m.normal_map.as_ref(), Some(&normal));
        assert!(m.height_map.is_none());
        assert_eq!(mats.len(), 2, "a bound texture is part of the sharing key");
    }
}
