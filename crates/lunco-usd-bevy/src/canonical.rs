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
        Self {
            stage,
            scene_layer: scene_layer.into(),
            runtime_layer: String::new(),
            inbox,
            sink_id,
            generation: 0,
        }
    }

    /// Build a `CanonicalStage` from a fetched [`StageRecipe`] — the main-thread
    /// build path for the runtime scene load (async fetch → this).
    pub fn from_recipe(recipe: &StageRecipe) -> anyhow::Result<Self> {
        let stage = crate::compose::build_stage_from_closure(recipe)?;
        Ok(Self::from_stage(stage, recipe.root_id.clone()))
    }

    /// A [`StageView`] over the composed stage for typed reads.
    pub fn view(&self) -> StageView<'_> {
        StageView::new(&self.stage)
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
