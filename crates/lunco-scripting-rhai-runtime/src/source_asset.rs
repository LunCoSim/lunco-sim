//! Script source assets as Bevy `Asset`s.
//!
//! Symmetric to `lunco_modelica_runtime::source_asset::ModelicaSource`. Domain
//! code must route `.rhai` reads through `AssetServer::load(...)` rather
//! than `std::fs::read_to_string` — that path doesn't exist on wasm32.
//! See `docs/architecture/40-asset-io.md`.

#[cfg(feature = "rhai")]
use bevy::asset::AssetPath;
#[cfg(feature = "rhai")]
use bevy::asset::{Asset, AssetLoader, LoadContext, io::Reader};
#[cfg(feature = "rhai")]
use bevy::prelude::*;
#[cfg(feature = "rhai")]
use std::collections::{BTreeMap, HashMap};

/// Raw text of a `.rhai` file — the file-backed twin of
/// [`lunco_core::EmbeddedScenarioSource`] (inline `info:sourceCode`). Lets a scene
/// reference a scenario by `info:sourceAsset` and keep the source as an
/// editable, hot-reloadable `.rhai` file instead of a string baked into USD.
#[cfg(feature = "rhai")]
#[derive(Asset, TypePath, Debug, Clone)]
pub struct RhaiSource {
    /// Raw `.rhai` text. UTF-8.
    pub text: String,
    /// Handles for every literal import in this source. Bevy keeps the source
    /// asset pending until this graph is loaded, so synchronous Rhai resolution
    /// never needs a discovery scan or a per-tick async bridge.
    #[dependency]
    pub dependencies: Vec<Handle<RhaiSource>>,
}

#[cfg(feature = "rhai")]
#[derive(Default, TypePath)]
pub struct RhaiSourceLoader;

/// Schedule boundary for publishing loaded Rhai source into the synchronous
/// import registry. Consumers that compile file-backed programs from an asset
/// event must run after this set so the dependency graph is visible to the
/// resolver.
#[cfg(feature = "rhai")]
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RhaiSourceAssetSet;

#[cfg(feature = "rhai")]
impl AssetLoader for RhaiSourceLoader {
    type Asset = RhaiSource;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let text = String::from_utf8(bytes)?;
        let importer = lunco_assets_core::asset_path::anchor_of(load_context.path());
        let dependencies = load_import_dependencies(&text, &importer, load_context)?;
        Ok(RhaiSource { text, dependencies })
    }

    fn extensions(&self) -> &[&str] {
        &["rhai"]
    }
}

#[cfg(feature = "rhai")]
fn load_import_dependencies(
    source: &str,
    importer: &str,
    load_context: &mut LoadContext<'_>,
) -> Result<Vec<Handle<RhaiSource>>, anyhow::Error> {
    import_dependency_ids(source, importer).map(|ids| {
        ids.into_iter()
            .map(|id| load_context.load(AssetPath::parse(&id).into_owned()))
            .collect()
    })
}

#[cfg(feature = "rhai")]
fn import_dependency_ids(source: &str, importer: &str) -> Result<Vec<String>, anyhow::Error> {
    lunco_scripting_rhai_core::module_resolver::imported_paths(source)
        .map_err(|error| anyhow::anyhow!("cannot inspect Rhai imports in {importer}: {error}"))
        .map(|paths| {
            paths
                .into_iter()
                .map(|path| {
                    lunco_assets_core::script_source::ScriptSources::canonical_id(
                        &path,
                        Some(importer),
                        "rhai",
                    )
                })
                .collect()
        })
}

/// Handles for application-owned Rhai sources discovered from the runtime
/// asset manifest. Keeping the handles alive makes the Bevy asset graph retain
/// the sources and lets the scripting runtime install edits without a compiled
/// snapshot. The manifest's extension is the only Rust-side selection rule;
/// source roles are decided by authored policy.
#[cfg(feature = "rhai")]
#[derive(Resource, Default)]
pub(crate) struct BuiltinRhaiAssets {
    pub(crate) handles: BTreeMap<String, Handle<RhaiSource>>,
    pub(crate) processed: HashMap<String, (String, u64)>,
}

/// Discover and request every authored Rhai source from the authoritative asset
/// manifest. This also works when the manifest arrives asynchronously on wasm;
/// no directory scan or compiled file list is required.
#[cfg(feature = "rhai")]
fn request_builtin_rhai_assets(
    manifest: Option<Res<lunco_assets_core::discovery::AssetManifest>>,
    asset_server: Option<Res<AssetServer>>,
    mut builtins: ResMut<BuiltinRhaiAssets>,
) {
    let Some(manifest) = manifest else {
        return;
    };
    if !manifest.ready() {
        return;
    }
    let Some(asset_server) = asset_server else {
        warn_once!("[rhai] built-in sources cannot load: AssetServer is not installed");
        return;
    };

    for rel in manifest.rels().iter().filter(|rel| rel.ends_with(".rhai")) {
        let rel = rel.clone();
        if !rel.ends_with(".rhai") {
            continue;
        }
        builtins
            .handles
            .entry(rel.clone())
            .or_insert_with(|| asset_server.load::<RhaiSource>(rel.clone()));
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
/// (`lunco_assets_core::asset_path::anchor_of`) — the same identity the `AssetServer`
/// loaded it under — so a script is importable by exactly the path that names it,
/// through whatever source it came from: `lunco://`, `twin://` for a campaign repo
/// outside the engine tree, or a peer's synced content mounted as a Twin.
///
/// The root scenario handle is owned by the scenario entity's
/// [`crate::commands::ScenarioAssetHandle`], while imported handles are retained
/// by `RhaiSource.dependencies`. An asset whose whole dependency chain has been
/// dropped is removed from the synchronous registry by its `Unused` event.
#[cfg(feature = "rhai")]
fn publish_rhai_sources(
    mut events: MessageReader<AssetEvent<RhaiSource>>,
    assets: Res<Assets<RhaiSource>>,
    asset_server: Res<AssetServer>,
    sources: Res<lunco_assets_core::script_source::ScriptSources>,
    mut registry: ResMut<lunco_scripting::ScriptRegistry>,
) {
    for ev in events.read() {
        match ev {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                // The root and every dependency are held by real ECS owners. A
                // missing value here is an engine lifecycle violation, not a cache
                // miss to paper over.
                let Some(src) = assets.get(*id) else {
                    warn!(
                        "[rhai] change event for {id:?} but the asset is gone; \
                         it is not importable or hot-reloadable"
                    );
                    continue;
                };
                let Some(path) = asset_server.get_path(*id) else {
                    warn!("[rhai] loaded script {id:?} has no asset path — not importable");
                    continue;
                };
                let canonical = lunco_assets_core::asset_path::anchor_of(&path);
                debug!("[rhai] script available for import: {canonical}");
                publish_rhai_source(&canonical, &src.text, &sources, &mut registry);
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                let Some(path) = asset_server.get_path(*id) else {
                    warn!(
                        "[rhai] unloaded script {id:?} has no asset path; \
                         its registry entry cannot be retired"
                    );
                    continue;
                };
                let canonical = lunco_assets_core::asset_path::anchor_of(&path);
                if sources.remove(&canonical) {
                    debug!("[rhai] retired script source: {canonical}");
                }
            }
            _ => {}
        }
    }
}

/// Publish one authoritative asset revision to both consumers of Rhai source.
///
/// `ScriptSources` serves synchronous `import` resolution. `ScriptRegistry`
/// serves already-attached scenario programs. They deliberately share the same
/// canonical asset identity: when Bevy reports a changed asset, every live
/// document carrying that identity must advance to those bytes. Advancing the
/// document generation is the normal scenario invalidation mechanism, so the
/// lifecycle driver performs the existing `on_stop -> compile -> on_start`
/// transition and the content-addressed compile cache naturally selects the new
/// program.
#[cfg(feature = "rhai")]
fn publish_rhai_source(
    canonical: &str,
    text: &str,
    sources: &lunco_assets_core::script_source::ScriptSources,
    registry: &mut lunco_scripting::ScriptRegistry,
) {
    sources.insert(canonical, text);

    let docs: Vec<_> = registry
        .documents
        .iter()
        .filter(|(_, host)| {
            host.document().asset_id.as_deref() == Some(canonical) && host.document().source != text
        })
        .map(|(id, _)| *id)
        .collect();
    let mut replaced = 0usize;
    for doc in docs {
        let before = registry
            .documents
            .get(&doc)
            .map(|host| host.generation())
            .unwrap_or_default();
        // Asset events are the external-source side of the existing document
        // lifecycle. The registry refresh replaces the clean base and advances
        // its generation; it is not an editor mutation and therefore does not
        // mint a second source ownership path or pollute undo history.
        if registry.reload_external_source(doc, text)
            && registry
                .documents
                .get(&doc)
                .is_some_and(|host| host.generation() != before)
        {
            replaced += 1;
        }
    }

    if replaced != 0 {
        info!("[rhai] replaced {replaced} running scenario program(s) from asset {canonical}");
    }
}

/// Plugin that registers the `.rhai` asset loader. Pulled in by
/// [`crate::LunCoScriptingRhaiRuntimePlugin`].
#[cfg(feature = "rhai")]
pub struct RhaiSourceAssetPlugin;

#[cfg(feature = "rhai")]
impl Plugin for RhaiSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RhaiSource>()
            .init_asset_loader::<RhaiSourceLoader>()
            .init_resource::<BuiltinRhaiAssets>()
            .add_systems(Update, request_builtin_rhai_assets)
            .add_systems(
                Update,
                (publish_rhai_sources,)
                    .in_set(RhaiSourceAssetSet)
                    .run_if(resource_exists::<lunco_assets_core::script_source::ScriptSources>),
            );
    }
}

#[cfg(all(test, feature = "rhai"))]
mod tests {
    use super::*;

    #[test]
    fn imported_assets_use_the_importers_canonical_source() {
        assert_eq!(
            import_dependency_ids(
                r#"import "helpers" as helpers; import "/scripting/lib/shots" as shots;"#,
                "twin://mission/main.rhai",
            )
            .unwrap(),
            ["twin://mission/helpers.rhai", "scripting/lib/shots.rhai"]
        );
    }
}
