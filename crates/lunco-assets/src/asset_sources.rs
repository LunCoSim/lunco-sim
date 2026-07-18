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

/// A custom asset **scheme** contributed to the LunCo asset-source registry.
///
/// Any crate registers one with `inventory::submit!` so scheme ownership lives
/// with the crate that implements the reader — no central hardcoded list. The
/// registrar ([`register_lunco_asset_sources`]) drains every submitted provider
/// **before** `AssetPlugin` builds (Bevy snapshots asset sources there):
///
/// ```ignore
/// inventory::submit! {
///     lunco_assets::AssetSchemeProvider { scheme: "scenario", build: scenario_asset_source }
/// }
/// ```
///
/// `build` is a bare `fn` (not a closure): inventory items are `'static` with no
/// captured state. A reader that needs shared *runtime* state (e.g. `twin://`'s
/// `TwinRoots`, shared with a resource) can't be a stateless provider and is
/// registered explicitly by the registrar instead.
///
/// TODO(interactive): schemes are fixed at startup because Bevy freezes the
/// asset-source registry at `AssetPlugin` build. To make schemes *runtime-
/// dynamic* and scriptable (rhai), add a single dispatcher source
/// (`lunco://<handler>/…`) backed by a `SchemeRegistry` resource whose handlers
/// (path → bytes, or redirect to an existing source) can be added at runtime and
/// from scripts. This link-time registry stays the path for crate-owned schemes;
/// the dispatcher would layer on top for dynamic/scripted aliases.
pub struct AssetSchemeProvider {
    /// The scheme name, e.g. `"scenario"` → `scenario://…`.
    pub scheme: &'static str,
    /// Builds the source's reader. Called once, before `AssetPlugin`.
    pub build: fn() -> AssetSourceBuilder,
}

inventory::collect!(AssetSchemeProvider);

/// Register every LunCo asset source on `app` and insert the shared
/// [`TwinRoots`] resource. Idempotent per app; call once before `DefaultPlugins`.
///
/// | Scheme | Resolves to | Notes |
/// |---|---|---|
/// | `cached_textures://` | texture cache dir | processed textures |
/// | `lunco://` | `<cwd>/assets`, then `<cache>` | the engine asset *library* (rovers, parts, downloaded binaries) |
/// | `twin://<name>/…` | open Twin roots | external Twin scenes — **native fs**; web = TODO http |
/// | `scenario://<id>/…` | `<cache_dir>/scenarios/<id>` | downloaded scenario assets (native + web OPFS) — contributed via the registry |
///
/// The first three (engine-critical, path-derived) are registered explicitly so
/// web asset loading never depends on the collection mechanism. Every crate-
/// contributed scheme (via [`AssetSchemeProvider`] + `inventory::submit!`) is
/// drained in between; `scenario://` is contributed that way by this crate.
/// `twin://` is registered explicitly (stateful `TwinRoots`, native-only).
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

    // Crate-contributed schemes: every `inventory::submit!`d `AssetSchemeProvider`
    // is registered here with no edit to this function. lunco-assets itself
    // contributes `scenario://` this way (see `scenario_source`); other crates can
    // add their own. Drained before `AssetPlugin` (this fn runs pre-DefaultPlugins).
    for provider in inventory::iter::<AssetSchemeProvider> {
        app.register_asset_source(provider.scheme, (provider.build)());
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry must actually collect submissions at link time (not just
    /// compile) — assert `scenario://` (contributed via `inventory::submit!` in
    /// `scenario_source`) shows up when the registrar drains the registry.
    #[test]
    fn contributed_schemes_are_collected() {
        let mut schemes: Vec<&str> = Vec::new();
        for provider in inventory::iter::<AssetSchemeProvider> {
            schemes.push(provider.scheme);
        }
        assert!(
            schemes.contains(&crate::scenario_source::SCENARIO_SCHEME),
            "scenario:// must be collected through the inventory scheme registry, got {schemes:?}",
        );
    }
}
