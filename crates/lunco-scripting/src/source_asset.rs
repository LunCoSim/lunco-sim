//! Python script source as a Bevy `Asset`.
//!
//! Symmetric to `lunco_modelica::source_asset::ModelicaSource`. Domain
//! code must route `.py` reads through `AssetServer::load(...)` rather
//! than `std::fs::read_to_string` — that path doesn't exist on wasm32.
//! See `docs/architecture/40-asset-io.md`.

use bevy::asset::{Asset, AssetLoader, LoadContext, io::Reader};
use bevy::prelude::*;

/// Raw text of a `.py` file.
///
/// We don't pre-compile the script in the loader. `lunco_scripting::python`
/// owns Py bytecode and the compile happens lazily on first execution,
/// driven by `ScriptDocument`. Keeping the asset as a string keeps the
/// loader cheap and lets non-python consumers (linters, AI assistants)
/// share the same handle.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct PythonSource {
    /// Raw `.py` text. UTF-8.
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct PythonSourceLoader;

impl AssetLoader for PythonSourceLoader {
    type Asset = PythonSource;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let text = String::from_utf8(bytes)?;
        Ok(PythonSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["py"]
    }
}

/// Plugin that registers the `.py` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
pub struct PythonSourceAssetPlugin;

impl Plugin for PythonSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<PythonSource>()
            .init_asset_loader::<PythonSourceLoader>();
    }
}

/// Raw text of a `.rhai` file — the file-backed twin of
/// [`lunco_core::EmbeddedScenarioSource`] (inline `lunco:script`). Lets a scene
/// reference a scenario by `lunco:scriptPath` and keep the source as an
/// editable, hot-reloadable `.rhai` file instead of a string baked into USD.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct RhaiSource {
    /// Raw `.rhai` text. UTF-8.
    pub text: String,
}

#[derive(Default, TypePath)]
pub struct RhaiSourceLoader;

impl AssetLoader for RhaiSourceLoader {
    type Asset = RhaiSource;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let text = String::from_utf8(bytes)?;
        Ok(RhaiSource { text })
    }

    fn extensions(&self) -> &[&str] {
        &["rhai"]
    }
}

/// Publish every loaded `.rhai` asset into the registry that backs `import`.
///
/// **Event-driven, not per-tick**: this wakes only when an asset actually appears
/// or changes, so the steady-state cost is nothing. That is also what makes
/// hot-reload fall out for free — `Modified` re-registers the new text, and the
/// resolver's memo (which stores the source it compiled) recompiles on the diff.
///
/// Registration is keyed by the asset's own canonical id
/// (`lunco_assets::asset_path::anchor_of`) — the same identity the `AssetServer`
/// loaded it under — so a script is importable by exactly the path that names it,
/// through whatever source it came from: `lunco://`, `twin://` for a campaign repo
/// outside the engine tree, or a peer's synced content mounted as a Twin.
///
/// This only works because loaders RETAIN their handles in
/// [`ScriptSources::retain`](lunco_assets::script_source::ScriptSources::retain).
/// An asset whose handle was dropped is already gone when the event arrives.
fn publish_rhai_sources(
    mut events: MessageReader<AssetEvent<RhaiSource>>,
    assets: Res<Assets<RhaiSource>>,
    asset_server: Res<AssetServer>,
    sources: Res<lunco_assets::script_source::ScriptSources>,
) {
    for ev in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = ev else {
            continue;
        };
        // Both misses are REPORTED, never skipped: a script that loads but fails to
        // register makes every `import` of it fail with "not found", which is
        // indistinguishable from the file not existing. A dropped handle is the
        // likely cause — see the note on `retain`.
        let Some(src) = assets.get(*id) else {
            warn!(
                "[rhai] change event for {id:?} but the asset is gone — its handle \
                 was dropped, so it is not importable and will not hot-reload"
            );
            continue;
        };
        let Some(path) = asset_server.get_path(*id) else {
            warn!("[rhai] loaded script {id:?} has no asset path — not importable");
            continue;
        };
        let canonical = lunco_assets::asset_path::anchor_of(&path);
        debug!("[rhai] script available for import: {canonical}");
        sources.insert(canonical, src.text.clone());
    }
}

/// Load every `.rhai` the project has, so any of them can be `import`ed.
///
/// A scenario script is loaded because a USD prim names it. A LIBRARY module is
/// named by nothing but an `import` inside another script — and `import` is
/// resolved synchronously, mid-tick, so it cannot start a load and wait. Something
/// has to have loaded it already.
///
/// That something is this, and it reuses the pipeline rather than adding one:
/// [`discovery::list_assets`] is the existing "which files exist" enumerator (the
/// spawn catalog uses it for `usda`, the shader catalog for `wgsl`), and it already
/// covers the engine library plus every open Twin — which is exactly the set a
/// script may import from. Loading goes through the ordinary `AssetServer`, so
/// every scheme works, including a peer's synced content.
///
/// Re-runs whenever the manifest changes (it lands late on the web) or a Twin is
/// opened, so a campaign repo mounted mid-session becomes importable without a
/// restart. Already-loaded paths are skipped by the registry's own residency.
fn preload_importable_scripts(
    asset_server: Res<AssetServer>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    roots: Res<lunco_assets::TwinRoots>,
    sources: Res<lunco_assets::script_source::ScriptSources>,
    mut seen: Local<std::collections::HashSet<String>>,
) {
    for file in lunco_assets::discovery::list_assets(&manifest, &roots, "rhai") {
        // Load through the ANCHORED uri, not the bare enumerated path. Discovery
        // reports engine-library files relative (`scenarios/lander_subsystems.rhai`), which the
        // `AssetServer` loads from the default source — a different `AssetPath` than
        // the `lunco://scenarios/lander_subsystems.rhai` a reference resolves to. Both
        // reach the same file, so the same script would register under two ids and
        // an author would have to guess which one `import` wants.
        let id = lunco_assets::asset_path::canonicalize_root(&file.asset_path);
        if !seen.insert(id.clone()) {
            continue;
        }
        // Retained, not merely loaded: a handle dropped on the floor takes the asset
        // with it, and the module would vanish before anything could import it.
        // `publish_rhai_sources` registers the text when the load lands.
        let handle: Handle<RhaiSource> = asset_server.load(id.clone());
        sources.retain(id, handle.untyped());
    }
}

/// Plugin that registers the `.rhai` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
pub struct RhaiSourceAssetPlugin;

impl Plugin for RhaiSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RhaiSource>()
            .init_asset_loader::<RhaiSourceLoader>()
            .add_systems(
                Update,
                (
                    // Discovery is change-driven: the manifest arrives late on the
                    // web, and Twins open at runtime.
                        preload_importable_scripts.run_if(
                            resource_exists_and_changed::<lunco_assets::discovery::AssetManifest>
                                // `exists_and_changed`, NOT `resource_changed`: a bare
                                // `resource_changed` PANICS the schedule when the
                                // resource is absent, and `TwinRoots` only exists once
                                // a twin-capable app inserts it (lunica crashed at
                                // startup on exactly this).
                                .or_else(resource_exists_and_changed::<lunco_assets::TwinRoots>),
                        ),
                    publish_rhai_sources,
                )
                    .chain()
                    .run_if(resource_exists::<lunco_assets::script_source::ScriptSources>),
            );
    }
}
