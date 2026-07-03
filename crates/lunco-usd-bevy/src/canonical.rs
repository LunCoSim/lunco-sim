//! `CanonicalStage` — the live composed openusd `Stage` as the single source of
//! truth for a scene (Ph0′ substrate).
//!
//! openusd's `Stage` is `Rc`-backed (`!Send`), so it lives as a **`NonSend`**
//! resource on the main thread; every USD read/author/project system runs on the
//! main thread and reads it through [`StageView`](crate::view::StageView). The
//! rest of the engine (render / physics / async) consumes the `Send` ECS
//! components the projection emits — never the stage. The stage is the membrane.
//!
//! A [`StageSink`] pushes each committed change into a `Send` inbox
//! (`Arc<Mutex<..>>`) that a projection system drains per tick; this is how live
//! edits (and reference-dependent cascade) reach the projector.
//!
//! S1 scope: build + hold the stage, expose a [`StageView`], and capture change
//! notices. The runtime/session edit-target sublayer, EditTarget authoring, and
//! the chunked physics-aware projector land in later slices (S2/S3).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use openusd::sdf::Path as SdfPath;
use openusd::usd::{CommittedChange, Stage, StageSinkId};

use crate::view::StageView;

/// A `Send` in-memory recipe for building a canonical [`Stage`]: the resolved
/// root layer identifier + the full transitive `.usda` layer-closure bytes
/// (from [`fetch_layer_closure`](crate::compose)). This crosses the async→main
/// boundary the `!Send` `Stage` cannot: an asset loader fetches the recipe
/// off-thread, a main-thread system builds the `CanonicalStage` from it.
#[derive(Clone)]
pub struct StageRecipe {
    pub root_id: String,
    pub bytes: HashMap<String, Vec<u8>>,
}

impl StageRecipe {
    /// A **single-layer** recipe from an in-memory `source` string — for scenes
    /// authored / composed in memory that carry no external file references
    /// (live documents, viewport preview, tests). `root_id` is a synthetic layer
    /// identifier (also the sole key in `bytes`), so `build_stage_from_closure`
    /// opens it straight from the byte map with no filesystem access. Sources
    /// with on-disk `references`/`payloads` need the full closure instead (they
    /// won't resolve from a lone in-memory layer).
    pub fn from_source(root_id: impl Into<String>, source: &str) -> Self {
        let root_id = root_id.into();
        let bytes = HashMap::from([(root_id.clone(), source.as_bytes().to_vec())]);
        Self { root_id, bytes }
    }
}

/// One committed change, owned + `Send`, as drained from the stage sink.
/// (`CommittedChange` borrows the stage; we copy the paths out so the inbox can
/// cross the sink→system boundary.)
#[derive(Debug, Clone, Default)]
pub struct RawStageChange {
    /// Structurally-resynced prim paths — includes reference/sublayer dependents
    /// that PCP fanned out (cascade), so the projector re-reads exactly these.
    pub resynced: Vec<SdfPath>,
    /// Attribute-only ("info only") prim paths — cheap incremental projection.
    pub info_only: Vec<SdfPath>,
    /// Identifier of the layer whose edit produced this change.
    pub layer: String,
}

/// The canonical live-composed stage for the active scene. `NonSend` (holds an
/// `Rc`-backed `Stage`). Insert via `world.insert_non_send_resource(..)`.
pub struct CanonicalStage {
    stage: Stage,
    /// Root (persisted) layer identifier — the scene `.usda`.
    pub scene_layer: String,
    /// Ephemeral edit-target sublayer identifier (empty until S2 inserts it).
    pub runtime_layer: String,
    /// Sink inbox drained by the projection system each tick.
    inbox: Arc<Mutex<Vec<RawStageChange>>>,
    #[allow(dead_code)] // held to keep the sink alive for the stage's lifetime
    sink_id: StageSinkId,
    /// Precomputed binary (glTF) arc sites for `lunco:resolvedAsset` synthesis
    /// off the live stage (what `flatten_stage` does for the baked path).
    binary_sites: crate::compose::BinarySites,
    /// Bumped by the drain step on each observed change (debug / asserts).
    pub generation: u64,
}

impl CanonicalStage {
    /// Wrap an already-composed [`Stage`] (from `compose_to_stage` /
    /// `compose_file_to_stage`), installing the change sink. `scene_layer` is the
    /// root layer identifier the stage was opened from.
    pub fn from_stage(stage: Stage, scene_layer: impl Into<String>) -> Self {
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let sink_inbox = inbox.clone();
        let sink_id = stage.add_sink(move |_stage: &Stage, change: &CommittedChange<'_>| {
            if let Ok(mut q) = sink_inbox.lock() {
                q.push(RawStageChange {
                    resynced: change.resynced.to_vec(),
                    info_only: change.changed_info_only.to_vec(),
                    layer: change.layer_identifier.to_string(),
                });
            }
        });
        // Precompute the binary-arc sites once (glTF/DEM resolution) so the live
        // `resolved_asset` read doesn't rescan every layer per prim.
        let binary_sites = crate::compose::discover_binary_sites(&stage);
        Self {
            stage,
            scene_layer: scene_layer.into(),
            runtime_layer: String::new(),
            inbox,
            sink_id,
            binary_sites,
            generation: 0,
        }
    }

    /// Build a `CanonicalStage` from a fetched [`StageRecipe`] — the main-thread
    /// build path for the runtime scene load (async fetch → this).
    pub fn from_recipe(recipe: &StageRecipe) -> anyhow::Result<Self> {
        let stage = crate::compose::build_stage_from_closure(recipe)?;
        Ok(Self::from_stage(stage, recipe.root_id.clone()))
    }

    /// A [`StageView`] over the composed stage for typed reads — carrying the
    /// precomputed binary-arc sites so `resolved_asset` synthesizes glTF URIs.
    pub fn view(&self) -> StageView<'_> {
        StageView::with_binary_sites(&self.stage, &self.binary_sites)
    }

    /// The underlying stage (escape hatch for authoring / reads not yet wrapped).
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// Drain and clear the change inbox, bumping `generation` if anything landed.
    pub fn drain_changes(&mut self) -> Vec<RawStageChange> {
        let drained = self
            .inbox
            .lock()
            .map(|mut q| std::mem::take(&mut *q))
            .unwrap_or_default();
        if !drained.is_empty() {
            self.generation += 1;
        }
        drained
    }
}

/// The set of live canonical stages, keyed by the `UsdStageAsset` they were
/// built from — the runtime home of the Ph0′ canonical document. `NonSend`
/// (each `CanonicalStage` holds an `Rc`-backed `Stage`). Parallels
/// `Assets<UsdStageAsset>`: a consumer that has an entity's
/// `UsdPrimPath.stage_handle` can look up the matching live stage here.
#[derive(Default)]
pub struct CanonicalStages {
    by_asset: HashMap<bevy::asset::AssetId<crate::UsdStageAsset>, CanonicalStage>,
}

impl CanonicalStages {
    /// The live canonical stage built from `asset`, if any.
    pub fn get(&self, asset: bevy::asset::AssetId<crate::UsdStageAsset>) -> Option<&CanonicalStage> {
        self.by_asset.get(&asset)
    }

    pub fn get_mut(
        &mut self,
        asset: bevy::asset::AssetId<crate::UsdStageAsset>,
    ) -> Option<&mut CanonicalStage> {
        self.by_asset.get_mut(&asset)
    }


    pub fn len(&self) -> usize {
        self.by_asset.len()
    }

    /// Insert (or replace) the live stage for `asset` — the door the live-doc
    /// projection uses to publish a `CanonicalStage` it built from a document's
    /// composed source, so the extractors read the live stage in-app and the
    /// change sink is installed. Replacing drops the previous stage (and its
    /// sink) for that asset.
    pub fn insert(
        &mut self,
        asset: bevy::asset::AssetId<crate::UsdStageAsset>,
        stage: CanonicalStage,
    ) {
        self.by_asset.insert(asset, stage);
    }

    /// Drain the change-sink inbox of **every** live stage, returning the
    /// committed changes per asset (empty stages omitted). The Step-1 projection
    /// bridge calls this each tick, then reconciles ECS off each stage's live
    /// [`view`](CanonicalStage::view) — the read counterpart to authoring onto
    /// the stage. Draining bumps each affected stage's `generation`.
    pub fn drain_all_changes(
        &mut self,
    ) -> Vec<(bevy::asset::AssetId<crate::UsdStageAsset>, Vec<RawStageChange>)> {
        self.by_asset
            .iter_mut()
            .filter_map(|(id, cs)| {
                let changes = cs.drain_changes();
                (!changes.is_empty()).then(|| (*id, changes))
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.by_asset.is_empty()
    }

    /// Build the canonical stage for `asset` from its `recipe` **on demand** if
    /// not already present, and return a reference to it.
    ///
    /// Ph0′ timing fix: `sync_canonical_stages` reacts to `AssetEvent`s in
    /// `Update`, but the visual/physics extractors instantiate synchronously in
    /// the `on_usd_prim_added` observer cascade — which runs BEFORE that system
    /// in the load frame. So the extractors would always miss the live stage and
    /// fall back to the flatten. Building here, at the first read, makes the
    /// canonical stage the source of truth regardless of system ordering. Cached,
    /// so the whole prim cascade shares one composed stage. `None` only if the
    /// asset carries no `recipe` (legacy flatten-only construction) or the build
    /// fails.
    pub fn get_or_build(
        &mut self,
        asset: bevy::asset::AssetId<crate::UsdStageAsset>,
        recipe: &crate::StageRecipe,
    ) -> Option<&CanonicalStage> {
        if !self.by_asset.contains_key(&asset) {
            match CanonicalStage::from_recipe(recipe) {
                Ok(cs) => {
                    bevy::log::debug!(
                        "[canonical] on-demand built CanonicalStage for {asset:?} ({} prims)",
                        cs.view().prim_paths().len()
                    );
                    self.by_asset.insert(asset, cs);
                }
                Err(e) => {
                    bevy::log::warn!("[canonical] on-demand from_recipe failed for {asset:?}: {e}");
                    return None;
                }
            }
        }
        self.by_asset.get(&asset)
    }
}

/// Main-thread system (Ph0′): when a `UsdStageAsset` finishes loading with a
/// [`StageRecipe`], build its live [`CanonicalStage`] and stash it in
/// [`CanonicalStages`]. Additive — runs ALONGSIDE the flattened-asset path; the
/// legacy extractors are untouched until the S2e cutover. `NonSend` because the
/// built `Stage` is `!Send`, so this system is pinned to the main thread.
pub fn sync_canonical_stages(
    mut events: bevy::prelude::MessageReader<bevy::asset::AssetEvent<crate::UsdStageAsset>>,
    assets: bevy::prelude::Res<bevy::asset::Assets<crate::UsdStageAsset>>,
    mut stages: bevy::prelude::NonSendMut<CanonicalStages>,
) {
    use bevy::asset::AssetEvent;
    for event in events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let Some(asset) = assets.get(*id) else { continue };
                let Some(recipe) = asset.recipe.as_ref() else { continue };
                match CanonicalStage::from_recipe(recipe) {
                    Ok(cs) => {
                        bevy::log::info!(
                            "[canonical] built CanonicalStage for {:?} ({} prims)",
                            id,
                            cs.view().prim_paths().len()
                        );
                        stages.by_asset.insert(*id, cs);
                    }
                    Err(e) => {
                        bevy::log::warn!("[canonical] from_recipe failed for {id:?}: {e}");
                    }
                }
            }
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => {
                stages.by_asset.remove(id);
            }
            _ => {}
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod recipe_tests {
    //! Ph0′ S2d: the runtime build primitive — a `StageRecipe` (what the loader
    //! fetches off-thread) builds, on the main thread, a `CanonicalStage` whose
    //! composed reads match the known-good file-composed stage.

    use super::*;
    use crate::compose::compose_file_to_stage;
    use crate::view::StageView;

    const FIXTURE: &str = "#usda 1.0\n\ndef Xform \"Root\"\n{\n    def Cube \"Box\"\n    {\n        double size = 3\n    }\n}\n";

    #[test]
    fn from_recipe_builds_composed_stage() {
        let dir = std::env::temp_dir().join("lunco_recipe_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("scene.usda");
        std::fs::write(&f, FIXTURE).unwrap();

        // Recipe mirrors what `fetch_layer_closure` produces for a ref-less scene:
        // root keyed by the SAME canonical id the resolver uses.
        let root_id = crate::resolver::canonicalize(f.to_str().unwrap(), None);
        let bytes = HashMap::from([(root_id.clone(), std::fs::read(&f).unwrap())]);
        let recipe = StageRecipe { root_id, bytes };

        let cstage = CanonicalStage::from_recipe(&recipe).expect("from_recipe builds a stage");
        let view = cstage.view();
        let prims: Vec<String> = view.prim_paths().iter().map(|p| p.to_string()).collect();
        assert!(
            prims.iter().any(|p| p == "/Root/Box"),
            "recipe-built stage must contain /Root/Box, got {prims:?}"
        );
        assert_eq!(view.value::<f64>(&SdfPath::new("/Root/Box").unwrap(), "size"), Some(3.0));

        // And it composes identically to the known-good file-composed path.
        let ref_stage = compose_file_to_stage(&f).expect("file compose");
        let ref_prims: Vec<String> =
            StageView::new(&ref_stage).prim_paths().iter().map(|p| p.to_string()).collect();
        assert_eq!(prims, ref_prims, "recipe-built stage must match file-composed stage");
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod sync_system_tests {
    //! Ph0′ S2d-wiring: the `sync_canonical_stages` SYSTEM, in a minimal Bevy
    //! App, must turn a loaded `UsdStageAsset` (carrying a `StageRecipe`) into a
    //! live `CanonicalStage` in the `CanonicalStages` resource — the exact runtime
    //! path the headless server exercises on scene load.

    use super::*;
    use bevy::asset::{AssetApp, AssetPlugin};
    use bevy::prelude::*;
    use std::sync::Arc;

    const FIXTURE: &str =
        "#usda 1.0\n\ndef Xform \"Root\"\n{\n    def Cube \"Box\"\n    {\n        double size = 3\n    }\n}\n";

    #[test]
    fn sync_canonical_stages_builds_stage_from_loaded_asset() {
        let dir = std::env::temp_dir().join("lunco_sync_system_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("scene.usda");
        std::fs::write(&f, FIXTURE).unwrap();

        let root_id = crate::resolver::canonicalize(f.to_str().unwrap(), None);
        let bytes = HashMap::from([(root_id.clone(), std::fs::read(&f).unwrap())]);
        let recipe = StageRecipe { root_id, bytes };
        // A reader for the asset (the field the legacy path uses); the system
        // itself only reads `recipe`.
        let stage = crate::compose::build_stage_from_closure(&recipe).unwrap();
        let reader = Arc::new(crate::compose::flatten_stage(&stage).unwrap());

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<crate::UsdStageAsset>()
            .init_non_send_resource::<CanonicalStages>()
            .add_systems(Update, sync_canonical_stages);

        // `Assets::add` emits `AssetEvent::Added`, which the system reads.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<crate::UsdStageAsset>>()
            .add(crate::UsdStageAsset { reader, recipe: Some(recipe) });

        // One frame flushes the asset event; the next lets the system act on it.
        app.update();
        app.update();

        let stages = app
            .world()
            .get_non_send_resource::<CanonicalStages>()
            .expect("CanonicalStages resource present");
        assert_eq!(stages.len(), 1, "exactly one canonical stage built from the loaded asset");
        let cs = stages.get(handle.id()).expect("canonical stage keyed by the asset id");
        assert!(
            cs.view().prim_paths().iter().any(|p| p.to_string() == "/Root/Box"),
            "the runtime canonical stage exposes the composed scene"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod resolved_asset_tests {
    //! Ph0′ visual-cutover prerequisite: `resolved_asset` synthesized off the
    //! LIVE stage must equal the `lunco:resolvedAsset` `flatten_stage` bakes —
    //! the glTF/DEM URI the visual extractor needs. A scene → wrapper → glb
    //! payload (the Perseverance "usda→glb" shape).

    use super::*;
    use crate::compose::{build_stage_from_closure, flatten_stage};
    use crate::UsdRead;
    use openusd::sdf::Path as SdfPath;

    #[test]
    fn resolved_asset_synth_matches_flatten_on_live_stage() {
        // A prim carrying a glb payload — the binary arc `resolved_asset`
        // synthesizes its URI from. Built through the SAME storage-based recipe
        // resolver the async loader uses (which stubs binary assets), so this is
        // the production compose path, not the deleted native-fs shim.
        let scene = "#usda 1.0\ndef Xform \"Scene\"\n{\n    def Xform \"Visual\" (\n        prepend payload = @model.glb@\n    )\n    {\n        string lunco:assetMode = \"scene\"\n    }\n}\n";
        let recipe = StageRecipe::from_source("scene.usda", scene);
        let stage = build_stage_from_closure(&recipe).expect("live stage from recipe");
        let cs = CanonicalStage::from_stage(stage, "scene.usda");
        let flat = flatten_stage(cs.stage()).expect("flatten");

        let visual = SdfPath::new("/Scene/Visual").unwrap();
        let live = cs.view().resolved_asset(&visual);
        let baked = UsdRead::resolved_asset(&flat, &visual);
        assert_eq!(live, baked, "live resolved_asset synth must equal the flatten's");
        assert!(
            live.as_deref().is_some_and(|u| u.ends_with("model.glb")),
            "live stage must synthesize the glb URI on the composed prim, got {live:?}"
        );
    }
}
