//! One place that registers **all** LunCo Bevy asset sources, so every binary
//! (luncosim, sandbox, web, model_viewer) gets the *same* schemes instead
//! of each `main()` hand-listing a divergent subset.
//!
//! Asset sources must be registered **before** `AssetPlugin`/`DefaultPlugins`
//! builds (Bevy snapshots the source registry at that point), so call
//! [`register_lunco_asset_sources`] right after `App::new()`, before
//! `.add_plugins(DefaultPlugins)`.

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;

use crate::lunco_source::lunco_asset_source;
use crate::twin_source::{twin_asset_source, TwinRoots};
use crate::textures_dir;

/// Register every LunCo asset source on `app` and insert the shared
/// [`TwinRoots`] resource. Idempotent per app; call once before `DefaultPlugins`.
///
/// | Scheme | Resolves to | Notes |
/// |---|---|---|
/// | `cached_textures://` | texture cache dir | processed textures |
/// | `lunco://` | `<cwd>/assets`, then `<cache>` | the engine asset *library* (rovers, parts, downloaded binaries) |
/// | `twin://<name>/…` | open Twin roots | Twin scenes AND downloaded scenarios — native fs + web OPFS, via `lunco_storage` |
///
/// The first two are path-derived and stateless; `twin://` is separate only
/// because its reader is stateful (it shares [`TwinRoots`] with the resource).
///
/// A **downloaded scenario is just a Twin root** over its cache directory, so it
/// needs no scheme of its own: one `twin://<name>/<rel>` names the scene on every
/// peer regardless of where that peer's bytes live. That is what keeps
/// `Provenance::Content`-derived ids identical across host and client.
///
/// Returns the [`TwinRoots`] handle (already inserted as a resource) for callers
/// that want to pre-register a root before the first scene load.
pub fn register_lunco_asset_sources(app: &mut App) -> TwinRoots {
    let assets_dir = std::env::current_dir().unwrap_or_default().join("assets");

    app.register_asset_source(
        "cached_textures",
        AssetSourceBuilder::platform_default(&textures_dir().to_string_lossy(), None),
    )
    // Engine asset *library* under a NAMED, location-independent scheme so a
    // scene living OUTSIDE the project (an external Twin) can still reference
    // shared parts: `@lunco://vessels/rovers/skid_rover.usda@`.
    //
    // Resolves `assets/` FIRST, then the download cache — so a large binary
    // pulled by `cargo run -p lunco-assets -- download` is reachable at its
    // logical `lunco://` address without any authored file naming the cache.
    // (This replaced `lunco-lib://`, which addressed the cache directly and so
    // baked a machine-local location into shipped `.usda` files.)
    .register_asset_source("lunco", lunco_asset_source(&assets_dir));

    // `twin://` — a named root, keyed by Twin name: an open Twin's directory, or a
    // downloaded scenario's cache dir. Registered on EVERY platform; the reader
    // goes through `lunco_storage`, so on web it reads the OPFS tree.
    let twin_roots = TwinRoots::default();
    app.register_asset_source("twin", twin_asset_source(&twin_roots));
    app.insert_resource(twin_roots.clone());
    twin_roots
}
