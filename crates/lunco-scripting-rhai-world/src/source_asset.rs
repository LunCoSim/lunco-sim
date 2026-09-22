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
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[cfg(feature = "rhai")]
use crate::tool_libs::ScriptSourceRole;

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
                    lunco_assets_runtime::script_source::ScriptSources::canonical_id(
                        &path,
                        Some(importer),
                        "rhai",
                    )
                })
                .collect()
        })
}

/// Handles for application-owned Rhai sources selected by the authored startup
/// classification policy from the runtime asset manifest. Keeping the handles
/// alive makes the Bevy asset graph retain the sources and lets the scripting
/// runtime install edits without a compiled snapshot. The manifest supplies
/// candidates; the policy decides which sources belong to the startup runtime.
#[cfg(feature = "rhai")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessedRhaiSource {
    pub(crate) text: String,
    pub(crate) role: Option<ScriptSourceRole>,
}

#[cfg(feature = "rhai")]
#[derive(Resource, Default)]
pub struct BuiltinRhaiAssets {
    pub(crate) handles: BTreeMap<String, Handle<RhaiSource>>,
    pub(crate) processed: HashMap<String, ProcessedRhaiSource>,
    /// Monotonic admission revision. The source policy is evaluated only when
    /// the manifest or its hook implementation changes, not once per update.
    pub(crate) admission_revision: u64,
    pub(crate) prepared_revision: u64,
    pub(crate) prepared_asset_revision: u64,
    manifest_revision: u64,
    policy_generation: u64,
}

/// Monotonic asset-event revision consumed by the application-level prelude
/// preparation pass. Publishing remains event-driven; consumers can use this
/// revision as a cheap run condition without each owning another message scan.
#[cfg(feature = "rhai")]
#[derive(Resource, Default)]
pub struct RhaiSourceAssetRevision(pub(crate) u64);

/// Discover authored Rhai candidates from the authoritative asset manifest and
/// request only sources that the startup classification policy admits. This
/// also works when the manifest arrives asynchronously on wasm; no directory
/// scan or compiled file list is required. Explicit scenario and scene loads
/// retain their own asset ownership path and are not admitted here implicitly.
#[cfg(feature = "rhai")]
fn request_builtin_rhai_assets(
    manifest: Option<Res<lunco_assets_runtime::discovery::AssetManifest>>,
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

    let policy_generation = lunco_hooks::generation();
    let manifest_revision = manifest.revision();
    if builtins.admission_revision != 0
        && builtins.manifest_revision == manifest_revision
        && builtins.policy_generation == policy_generation
    {
        return;
    }
    builtins.manifest_revision = manifest_revision;
    builtins.policy_generation = policy_generation;
    builtins.admission_revision = builtins.admission_revision.wrapping_add(1);

    // The manifest is only an inventory. The authored policy owns the
    // extension and path decision, so this loop does not grow a Rust-side
    // allow-list as new source classes are authored.
    let mut admitted_ids = BTreeSet::new();
    for rel in manifest.rels() {
        let rel = rel.clone();
        let admitted = match crate::tool_libs::classify_source(&rel) {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(error) => {
                // Keep the candidate alive so `prepare_builtin_rhai_assets`
                // reports the typed classification failure and closes the
                // runtime. A malformed policy result must not disappear as
                // if the source had been intentionally ignored.
                warn_once!("[rhai] startup source classification rejected `{rel}`: {error}");
                true
            }
        };
        if !admitted {
            continue;
        }
        admitted_ids.insert(rel.clone());
        builtins.handles.entry(rel.clone()).or_insert_with(|| {
            asset_server.load::<RhaiSource>(lunco_assets_core::engine_asset_uri(&rel))
        });
    }

    // Policy replacement is a real lifecycle change: release built-in handles
    // that the new policy no longer admits. Explicit scenario handles are
    // owned by their requesting entities and are unaffected by this pruning.
    builtins.handles.retain(|rel, _| admitted_ids.contains(rel));
    // Keep processed entries for one preparation pass. That pass owns the
    // standard-tool retirement after it can compare the old role with the new
    // policy result; dropping the record here would leave a retired tool in
    // the visible registry.
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
    sources: Res<lunco_assets_runtime::script_source::ScriptSources>,
    mut registry: ResMut<lunco_scripting::ScriptRegistry>,
    mut revision: ResMut<RhaiSourceAssetRevision>,
) {
    let mut changed = false;
    for ev in events.read() {
        match ev {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                changed = true;
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
                changed = true;
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
            AssetEvent::LoadedWithDependencies { .. } => changed = true,
        }
    }
    if changed {
        revision.0 = revision.0.wrapping_add(1);
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
    sources: &lunco_assets_runtime::script_source::ScriptSources,
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
            .init_resource::<RhaiSourceAssetRevision>()
            .add_systems(Update, request_builtin_rhai_assets)
            .add_systems(
                Update,
                (publish_rhai_sources,)
                    .in_set(RhaiSourceAssetSet)
                    .run_if(resource_exists::<lunco_assets_runtime::script_source::ScriptSources>),
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
