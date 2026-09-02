//! Python script source as a Bevy `Asset`.
//!
//! Symmetric to `lunco_modelica::source_asset::ModelicaSource`. Domain
//! code must route `.py` reads through `AssetServer::load(...)` rather
//! than `std::fs::read_to_string` — that path doesn't exist on wasm32.
//! See `docs/architecture/40-asset-io.md`.

#[cfg(feature = "rhai")]
use bevy::asset::AssetPath;
use bevy::asset::{io::Reader, Asset, AssetLoader, LoadContext};
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
        let importer = lunco_assets::asset_path::anchor_of(load_context.path());
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
    crate::module_resolver::imported_paths(source)
        .map_err(|error| anyhow::anyhow!("cannot inspect Rhai imports in {importer}: {error}"))
        .map(|paths| {
            paths
                .into_iter()
                .map(|path| {
                    lunco_assets::script_source::ScriptSources::canonical_id(
                        &path,
                        Some(importer),
                        "rhai",
                    )
                })
                .collect()
        })
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
/// The root scenario handle is owned by the scenario entity's
/// [`crate::commands::ScenarioAssetHandle`], while imported handles are retained
/// by `RhaiSource.dependencies`. An asset whose whole dependency chain has been
/// dropped is removed from the synchronous registry by its `Unused` event.
#[cfg(feature = "rhai")]
fn publish_rhai_sources(
    mut events: MessageReader<AssetEvent<RhaiSource>>,
    assets: Res<Assets<RhaiSource>>,
    asset_server: Res<AssetServer>,
    sources: Res<lunco_assets::script_source::ScriptSources>,
    mut registry: ResMut<crate::ScriptRegistry>,
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
                let canonical = lunco_assets::asset_path::anchor_of(&path);
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
                let canonical = lunco_assets::asset_path::anchor_of(&path);
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
    sources: &lunco_assets::script_source::ScriptSources,
    registry: &mut crate::ScriptRegistry,
) {
    sources.insert(canonical, text);

    let mut replaced = 0usize;
    for host in registry.documents.values_mut() {
        let matches_asset = host.document().asset_id.as_deref() == Some(canonical);
        if !matches_asset || host.document().source == text {
            continue;
        }

        let before = host.generation();
        // Asset events are the external-source side of the existing document
        // lifecycle. `reload_base` replaces the clean base and advances its
        // generation; it is not an editor mutation and therefore does not mint a
        // second source ownership path or pollute undo history.
        if lunco_doc::FileBacked::reload_base(host.document_mut(), text)
            && host.generation() != before
        {
            replaced += 1;
        }
    }

    if replaced != 0 {
        info!("[rhai] replaced {replaced} running scenario program(s) from asset {canonical}");
    }
}

/// Plugin that registers the `.rhai` asset loader. Pulled in by
/// `LunCoScriptingPlugin`.
#[cfg(feature = "rhai")]
pub struct RhaiSourceAssetPlugin;

#[cfg(feature = "rhai")]
impl Plugin for RhaiSourceAssetPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<RhaiSource>()
            .init_asset_loader::<RhaiSourceLoader>()
            .add_systems(
                Update,
                (publish_rhai_sources,)
                    .in_set(RhaiSourceAssetSet)
                    .run_if(resource_exists::<lunco_assets::script_source::ScriptSources>),
            );
    }
}

#[cfg(all(test, feature = "rhai"))]
mod tests {
    use super::*;
    use crate::doc::{ScriptDocument, ScriptLanguage};
    use lunco_doc::DocumentId;

    fn document(id: u64, source: &str, asset_id: Option<&str>) -> ScriptDocument {
        let mut doc = ScriptDocument::new(id, ScriptLanguage::Rhai, source);
        doc.asset_id = asset_id.map(str::to_owned);
        doc
    }

    #[test]
    fn asset_revision_replaces_every_matching_live_document() {
        let sources = lunco_assets::script_source::ScriptSources::default();
        let mut registry = crate::ScriptRegistry::default();
        registry.insert_document(
            DocumentId::new(1),
            document(1, "fn on_start(me) {}", Some("lunco://scenario.rhai")),
        );
        registry.insert_document(
            DocumentId::new(2),
            document(2, "fn on_start(me) {}", Some("lunco://scenario.rhai")),
        );
        registry.insert_document(DocumentId::new(3), document(3, "inline", None));
        registry.insert_document(
            DocumentId::new(4),
            document(4, "other", Some("lunco://other.rhai")),
        );

        publish_rhai_source(
            "lunco://scenario.rhai",
            "fn on_start(me) { print(\"v2\"); }",
            &sources,
            &mut registry,
        );

        assert_eq!(
            sources.get("lunco://scenario.rhai").as_deref(),
            Some("fn on_start(me) { print(\"v2\"); }")
        );
        for id in [1, 2] {
            let doc = registry.documents[&DocumentId::new(id)].document();
            assert_eq!(doc.source, "fn on_start(me) { print(\"v2\"); }");
            assert_eq!(doc.generation, 1);
            assert_eq!(doc.last_saved_generation, Some(1));
        }
        assert_eq!(
            registry.documents[&DocumentId::new(3)].document().source,
            "inline"
        );
        assert_eq!(
            registry.documents[&DocumentId::new(4)].document().source,
            "other"
        );
    }

    #[test]
    fn identical_asset_revision_does_not_advance_generation() {
        let source = "fn on_start(me) {}";
        let sources = lunco_assets::script_source::ScriptSources::default();
        let mut registry = crate::ScriptRegistry::default();
        registry.insert_document(
            DocumentId::new(1),
            document(1, source, Some("lunco://scenario.rhai")),
        );

        publish_rhai_source("lunco://scenario.rhai", source, &sources, &mut registry);

        assert_eq!(
            registry.documents[&DocumentId::new(1)]
                .document()
                .generation,
            0
        );
    }

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
