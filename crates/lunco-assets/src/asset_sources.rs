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

use crate::twin_source::{twin_asset_source, TwinRoots};
use crate::{cache_dir, textures_dir};

/// A custom asset **scheme** contributed to the LunCo asset-source registry.
///
/// Any crate registers one with `inventory::submit!` so scheme ownership lives
/// with the crate that implements the reader — no central hardcoded list. The
/// registrar ([`register_lunco_asset_sources`]) drains every submitted provider
/// **before** `AssetPlugin` builds (Bevy snapshots asset sources there):
///
/// ```ignore
/// inventory::submit! {
///     lunco_assets::AssetSchemeProvider { scheme: "myscheme", build: my_asset_source }
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
    /// The scheme name, e.g. `"myscheme"` → `myscheme://…`.
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
/// | `lunco-lib://` | shared cache dir | shipped/downloaded fixtures (glTF models) |
/// | `lunco://` | `<cwd>/assets` | the engine asset *library* (rovers, parts) |
/// | `twin://<name>/…` | open Twin roots | Twin scenes AND downloaded scenarios — native fs + web OPFS, via `lunco_storage` |
///
/// The first three (engine-critical, path-derived) are registered explicitly so
/// web asset loading never depends on the collection mechanism. Every crate-
/// contributed scheme (via [`AssetSchemeProvider`] + `inventory::submit!`) is
/// drained in between. `twin://` is registered explicitly because its reader is
/// stateful (it shares [`TwinRoots`] with the resource), not because of any
/// platform limit.
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

    // Crate-contributed schemes: every `inventory::submit!`d `AssetSchemeProvider`
    // is registered here with no edit to this function, so scheme ownership can
    // live with the crate that implements the reader. Drained before `AssetPlugin`
    // (this fn runs pre-DefaultPlugins).
    for provider in inventory::iter::<AssetSchemeProvider> {
        app.register_asset_source(provider.scheme, (provider.build)());
    }

    // `twin://` — a named root, keyed by Twin name: an open Twin's directory, or a
    // downloaded scenario's cache dir. Registered on EVERY platform; the reader
    // goes through `lunco_storage`, so on web it reads the OPFS tree.
    let twin_roots = TwinRoots::default();
    app.register_asset_source("twin", twin_asset_source(&twin_roots));
    app.insert_resource(twin_roots.clone());
    twin_roots
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry must actually collect submissions at LINK time, not merely
    /// compile — a silently-empty `inventory` would drop a crate's scheme with no
    /// error, and the reader would simply never be registered.
    ///
    /// Contributes its own provider rather than asserting on a production scheme:
    /// the mechanism is what is under test, so it must not fail when the set of
    /// real schemes changes.
    fn test_scheme_source() -> AssetSourceBuilder {
        AssetSourceBuilder::platform_default("/tmp/lunco-test-scheme", None)
    }

    inventory::submit! {
        AssetSchemeProvider { scheme: "lunco-test-scheme", build: test_scheme_source }
    }

    #[test]
    fn contributed_schemes_are_collected() {
        let schemes: Vec<&str> = inventory::iter::<AssetSchemeProvider>
            .into_iter()
            .map(|p| p.scheme)
            .collect();
        assert!(
            schemes.contains(&"lunco-test-scheme"),
            "a submitted scheme must be collected through the inventory registry, got {schemes:?}",
        );
    }
}
