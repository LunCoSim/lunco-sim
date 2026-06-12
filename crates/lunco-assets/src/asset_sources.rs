//! One place that registers **all** LunCo Bevy asset sources, so every binary
//! (lunco-client, sandbox, web, model_viewer) gets the *same* schemes instead
//! of each `main()` hand-listing a divergent subset.
//!
//! Asset sources must be registered **before** `AssetPlugin`/`DefaultPlugins`
//! builds (Bevy snapshots the source registry at that point), so call
//! [`register_lunco_asset_sources`] right after `App::new()`, before
//! `.add_plugins(DefaultPlugins)`.

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;

use crate::twin_source::{twin_asset_source, TwinRoots};
use crate::{cache_dir, textures_dir};

/// Register every LunCo asset source on `app` and insert the shared
/// [`TwinRoots`] resource. Idempotent per app; call once before `DefaultPlugins`.
///
/// | Scheme | Resolves to | Notes |
/// |---|---|---|
/// | `cached_textures://` | texture cache dir | processed textures |
/// | `lunco-lib://` | shared cache dir | shipped/downloaded fixtures (glTF models) |
/// | `lunco://` | `<cwd>/assets` | the engine asset *library* (rovers, parts) |
/// | `twin://<name>/…` | open Twin roots | external Twin scenes — **native fs**; web = TODO http |
///
/// Returns the [`TwinRoots`] handle (already inserted as a resource) for callers
/// that want to pre-register a root before the first scene load.
pub fn register_lunco_asset_sources(app: &mut App) -> TwinRoots {
    let assets_dir = std::env::current_dir().unwrap_or_default().join("assets");

    app.register_asset_source(
        "cached_textures",
        AssetSourceBuilder::platform_default(&textures_dir().to_string_lossy(), None),
    )
    // Shipped/downloaded fixture library (glTF models), populated by
    // `cargo run -p lunco-assets -- download / process`.
    .register_asset_source(
        "lunco-lib",
        AssetSourceBuilder::platform_default(&cache_dir().to_string_lossy(), None),
    )
    // Engine asset *library* under a NAMED, location-independent scheme so a
    // scene living OUTSIDE the project (an external Twin) can still reference
    // shared parts: `@lunco://vessels/rovers/skid_rover.usda@`.
    .register_asset_source(
        "lunco",
        AssetSourceBuilder::platform_default(&assets_dir.to_string_lossy(), None),
    );

    // `twin://` — the open Twin's root, keyed by Twin name. The reader is
    // filesystem-backed, so it's native-only for now; the web port needs an
    // http-backed reader (TODO). The resource is inserted on every platform so
    // the Twin-open flow compiles uniformly.
    let twin_roots = TwinRoots::default();
    #[cfg(not(target_arch = "wasm32"))]
    app.register_asset_source("twin", twin_asset_source(&twin_roots));
    app.insert_resource(twin_roots.clone());
    twin_roots
}
