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
/// `Rc`-backed `Stage`). Insert via `world.insert_non_send(..)`.
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
    /// The live resolver's shared byte-map handle, when this stage was built
    /// from a [`StageRecipe`] via [`from_recipe`](Self::from_recipe). `Some`
    /// lets [`add_layer_bytes`](Self::add_layer_bytes) inject a spawned asset's
    /// layer closure so a subsequently [`author_reference`](Self::author_reference)d
    /// arc composes on the live stage (sink-driven referenced spawn). `None` for
    /// stages built via [`from_stage`](Self::from_stage) over a foreign resolver
    /// (native `compose_file_to_stage` / tests) — those can't gain layers.
    resolver_bytes: Option<crate::resolver::SharedLayerBytes>,
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
            resolver_bytes: None,
            generation: 0,
        }
    }

    /// Build a `CanonicalStage` from a fetched [`StageRecipe`] — the main-thread
    /// build path for the runtime scene load (async fetch → this). Captures the
    /// resolver's shared byte-map handle so runtime referenced spawns can inject
    /// their layer closure (see [`add_layer_bytes`](Self::add_layer_bytes)).
    pub fn from_recipe(recipe: &StageRecipe) -> anyhow::Result<Self> {
        let (stage, shared) = crate::compose::build_stage_with_resolver(recipe)?;
        let mut cs = Self::from_stage(stage, recipe.root_id.clone());
        cs.resolver_bytes = Some(shared);
        Ok(cs)
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

    /// Monotonic change counter — bumped whenever [`drain_changes`](Self::drain_changes)
    /// commits sink notices. A stage-reading projector (e.g. the policy projector)
    /// gates on this so it re-runs only when the composed stage actually changed.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Author `xformOp:translate = value` onto the composed prim at `path` (root
    /// edit target) — this fires the change sink, so the projection bridge
    /// ([`project_stage_changes`](crate::project_stage_changes) via
    /// `lunco-usd`) reconciles the move in place. Synthesizes `xformOpOrder` only
    /// when the prim composes none, so an existing xform stack is never
    /// clobbered.
    ///
    /// The `CanonicalStage` is the live **projection** (rebuilt from the recipe on
    /// a structural reload), so authoring here updates the live view + drives the
    /// sink WITHOUT touching the document's save data (`UsdDocument.base`, which
    /// stays the durable/serialized truth).
    pub fn author_translate(&self, path: &SdfPath, value: [f64; 3]) -> anyhow::Result<()> {
        use anyhow::anyhow;
        self.stage
            .create_attribute(format!("{}.xformOp:translate", path.as_str()), "double3")
            .map_err(|e| anyhow!("author translate at {path}: {e}"))?
            .set(value)
            .map_err(|e| anyhow!("set translate at {path}: {e}"))?;
        let has_order = self
            .stage
            .prim(path.clone())
            .attribute("xformOpOrder")
            .get::<openusd::sdf::Value>()
            .ok()
            .flatten()
            .is_some();
        if !has_order {
            let order = crate::author::parse_attribute_value("token[]", "[\"xformOp:translate\"]")
                .map_err(|e| anyhow!("xformOpOrder value: {e}"))?;
            self.stage
                .create_attribute(format!("{}.xformOpOrder", path.as_str()), "token[]")
                .map_err(|e| anyhow!("author xformOpOrder at {path}: {e}"))?
                .set(order)
                .map_err(|e| anyhow!("set xformOpOrder at {path}: {e}"))?;
        }
        Ok(())
    }

    /// Author `xformOp:rotateXYZ = value` (Euler XYZ, **degrees**) onto the
    /// composed prim at `path` — the rotation counterpart of
    /// [`author_translate`](Self::author_translate). Fires the change sink so the
    /// projection bridge reconciles the new orientation in place, and synthesizes
    /// `xformOpOrder` only when the prim composes none (never clobbers a stack).
    pub fn author_rotate(&self, path: &SdfPath, value: [f64; 3]) -> anyhow::Result<()> {
        use anyhow::anyhow;
        self.stage
            .create_attribute(format!("{}.xformOp:rotateXYZ", path.as_str()), "double3")
            .map_err(|e| anyhow!("author rotate at {path}: {e}"))?
            .set(value)
            .map_err(|e| anyhow!("set rotate at {path}: {e}"))?;
        let has_order = self
            .stage
            .prim(path.clone())
            .attribute("xformOpOrder")
            .get::<openusd::sdf::Value>()
            .ok()
            .flatten()
            .is_some();
        if !has_order {
            let order = crate::author::parse_attribute_value("token[]", "[\"xformOp:rotateXYZ\"]")
                .map_err(|e| anyhow!("xformOpOrder value: {e}"))?;
            self.stage
                .create_attribute(format!("{}.xformOpOrder", path.as_str()), "token[]")
                .map_err(|e| anyhow!("author xformOpOrder at {path}: {e}"))?
                .set(order)
                .map_err(|e| anyhow!("set xformOpOrder at {path}: {e}"))?;
        }
        Ok(())
    }

    /// Define a prim of `type_name` at `path` (root edit target) — fires the sink
    /// so the projection bridge spawns it. For a referenced spawn, follow with
    /// [`author_reference`](Self::author_reference).
    pub fn author_prim(&self, path: &SdfPath, type_name: Option<&str>) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let prim = self
            .stage
            .define_prim(path.clone())
            .map_err(|e| anyhow!("define_prim {path}: {e}"))?;
        if let Some(t) = type_name {
            prim.set_type_name(t).map_err(|e| anyhow!("set_type_name {path}: {e}"))?;
        }
        Ok(())
    }

    /// Remove the prim at `path` from the root edit target — fires the sink so the
    /// projection bridge despawns its subtree.
    pub fn remove_prim_at(&self, path: &SdfPath) -> anyhow::Result<bool> {
        use anyhow::anyhow;
        self.stage
            .remove_prim(path.clone())
            .map_err(|e| anyhow!("remove_prim {path}: {e}"))
    }

    /// Inject a spawned asset's layer closure (`id → bytes`, keyed by the same
    /// canonical id [`StageRecipe`] uses) into the live resolver, so a
    /// subsequently [`author_reference`](Self::author_reference)d arc to any of
    /// those ids composes on this stage. Returns `false` if this stage has no
    /// injectable resolver (built via [`from_stage`](Self::from_stage) over a
    /// foreign resolver). Merges — existing ids keep their bytes.
    pub fn add_layer_bytes(&self, extra: HashMap<String, Vec<u8>>) -> bool {
        match &self.resolver_bytes {
            Some(shared) => {
                shared.borrow_mut().extend(extra);
                true
            }
            None => false,
        }
    }

    /// Whether the live resolver already holds bytes for layer `id` — so a
    /// referenced spawn can skip the async fetch when its asset closure is
    /// already loaded (e.g. spawning a second rover of an already-referenced
    /// asset).
    pub fn has_layer_bytes(&self, id: &str) -> bool {
        self.resolver_bytes
            .as_ref()
            .map(|shared| shared.borrow().contains_key(id))
            .unwrap_or(false)
    }

    /// A snapshot clone of the live resolver's full layer-byte closure (every
    /// referenced `.usda` loaded so far). Lets the coarse `full_reload` path
    /// (Save-As / whole-source undo) rebuild a fresh stage from an edited root
    /// source that still references those layers, reusing the already-loaded
    /// closure so it recomposes without re-fetching. Empty if this stage has no
    /// injectable resolver.
    pub fn layer_bytes_snapshot(&self) -> HashMap<String, Vec<u8>> {
        self.resolver_bytes
            .as_ref()
            .map(|shared| shared.borrow().clone())
            .unwrap_or_default()
    }

    /// The canonical layer id an `asset_path` reference resolves to on *this*
    /// stage — `asset_path` anchored against the scene (root) layer, exactly as
    /// PCP will canonicalize the authored `references` arc. This is the key to
    /// load the asset closure under (`AssetServer::load` / [`add_layer_bytes`])
    /// so the injected bytes match what PCP demands.
    pub fn canonical_reference_id(&self, asset_path: &str) -> String {
        let anchor = openusd::ar::ResolvedPath::new(&self.scene_layer);
        crate::resolver::canonicalize(asset_path, crate::resolver::anchor_str(Some(&anchor)))
    }

    /// Author a `references = @asset_path@` arc onto the prim at `path` (root
    /// edit target), turning it into a **referenced spawn**: PCP composes the
    /// referenced asset's default prim under `path`. Fires the change sink so the
    /// projection bridge instantiates the composed subtree. The referenced
    /// asset's layer closure must already be resolvable — inject it first via
    /// [`add_layer_bytes`](Self::add_layer_bytes) (or it was loaded with the
    /// scene). openusd exposes no typed `add_reference`, so this authors the
    /// `references` field metadata directly (the live-stage counterpart of
    /// [`author::author_reference`](crate::author::author_reference), which
    /// writes the same field into the document's `sdf::Data`).
    pub fn author_reference(&self, path: &SdfPath, asset_path: &str) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let reference = openusd::sdf::Reference {
            asset_path: asset_path.to_string(),
            ..Default::default()
        };
        self.stage
            .prim(path.clone())
            .set_metadata(
                openusd::sdf::FieldKey::References.as_str(),
                openusd::sdf::Value::ReferenceListOp(openusd::sdf::ReferenceListOp::prepended(
                    [reference],
                )),
            )
            .map_err(|e| anyhow!("author reference @{asset_path}@ at {path}: {e}"))?;
        Ok(())
    }

    /// Author relationship `name` on `prim`, pointing at `targets` — the
    /// live-stage counterpart of the document's `SetRelationship` op.
    ///
    /// Without this, every relationship edit fell to the projector's whole-scene
    /// rebuild path. That is the difference between snapping a part onto an
    /// assembly and respawning every prim in the world: a physics joint authors
    /// `physics:body0` / `physics:body1`, so a component attach is *two*
    /// relationship edits. Set-semantics — `targets` replaces any prior list.
    pub fn author_relationship(
        &self,
        prim: &SdfPath,
        name: &str,
        targets: &[String],
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let target_paths = targets
            .iter()
            .map(|t| {
                openusd::sdf::Path::new(t)
                    .map_err(|e| anyhow!("relationship {prim}.{name}: bad target `{t}`: {e}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.stage
            .create_relationship(format!("{}.{}", prim.as_str(), name))
            .map_err(|e| anyhow!("author relationship {prim}.{name}: {e}"))?
            .set_targets(target_paths)
            .map_err(|e| anyhow!("set relationship targets {prim}.{name}: {e}"))?;
        Ok(())
    }

    /// Author attribute `name`'s connection targets (`connectionPaths`) onto the
    /// prim at `prim` — the live-stage counterpart of the document's
    /// `SetConnection` op. Creates the attribute spec if absent (like
    /// [`author_attribute`](Self::author_attribute)) so a wire can be drawn to a
    /// not-yet-materialised port. `sources` replaces any prior list; empty clears.
    ///
    /// The projector classified `SetConnection` as an incremental op but had no
    /// author for it, so the op reached the document and never the live stage —
    /// a silently dropped edit. Every cosim wire authored at runtime went
    /// nowhere until the next full rebuild.
    pub fn author_connection(
        &self,
        prim: &SdfPath,
        name: &str,
        type_name: &str,
        sources: &[String],
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        let source_paths = sources
            .iter()
            .map(|s| {
                openusd::sdf::Path::new(s)
                    .map_err(|e| anyhow!("connection {prim}.{name}: bad source `{s}`: {e}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.stage
            .create_attribute(format!("{}.{}", prim.as_str(), name), type_name)
            .map_err(|e| anyhow!("author connection {prim}.{name} ({type_name}): {e}"))?
            .set_connections(source_paths)
            .map_err(|e| anyhow!("set connection sources {prim}.{name}: {e}"))?;
        Ok(())
    }

    // NOTE: no live-stage `author_api_schemas` / `author_active`. Those two ops
    // change a prim's ECS component set / entity presence, which the incremental
    // subtree refresh (visual-only) can't reconcile — they take the projector's
    // rebuild path instead, which composes from the document (already carrying the
    // authored metadata). See `twin_projection::op_needs_rebuild`.

    /// Author attribute `name = value` (USD type `type_name`) onto the prim at
    /// `prim` (root edit target), firing the sink so the projection refreshes the
    /// prim's visual. Creates the attribute if absent, overwrites it otherwise —
    /// the live-stage counterpart of the document's `SetAttribute` op, so a
    /// material / inspector edit reaches the live world without a whole-scene
    /// reload. `value` is a typed [`openusd::sdf::Value`] (read from the composed
    /// document, or parsed via [`author::parse_attribute_value`](crate::author::parse_attribute_value)).
    pub fn author_attribute(
        &self,
        prim: &SdfPath,
        name: &str,
        type_name: &str,
        value: openusd::sdf::Value,
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        self.stage
            .create_attribute(format!("{}.{}", prim.as_str(), name), type_name)
            .map_err(|e| anyhow!("author attribute {prim}.{name} ({type_name}): {e}"))?
            .set(value)
            .map_err(|e| anyhow!("set attribute {prim}.{name}: {e}"))?;
        Ok(())
    }

    /// Author `name`'s `timeSamples[time] = value` (USD type `type_name`) onto the
    /// prim at `prim` (root edit target), firing the sink — the live-stage
    /// counterpart of the document's `SetTimeSample` op. A keyframe edit reaches
    /// the live world without a whole-scene rebuild: the per-frame animation
    /// sampler ([`sample_usd_animation`](crate::sample_usd_animation)) reads this
    /// stage each frame, so a key on an already-animated prim shows up on the next
    /// tick. Creates the attribute if absent; adds or overwrites the sample at
    /// `time` otherwise (openusd exposes no live-stage sample *removal*, so
    /// `RemoveTimeSample` stays on the projector's rebuild path).
    pub fn author_time_sample(
        &self,
        prim: &SdfPath,
        name: &str,
        type_name: &str,
        time: f64,
        value: openusd::sdf::Value,
    ) -> anyhow::Result<()> {
        use anyhow::anyhow;
        self.stage
            .create_attribute(format!("{}.{}", prim.as_str(), name), type_name)
            .map_err(|e| anyhow!("author time sample {prim}.{name} ({type_name}): {e}"))?
            .set_at(value, openusd::usd::TimeCode::new(time))
            .map_err(|e| anyhow!("set time sample {prim}.{name} @ {time}: {e}"))?;
        Ok(())
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

    /// Iterate every live stage keyed by its asset id — the door a whole-stage
    /// projector (e.g. the policy projector, which reads composed `LunCoPolicy`
    /// prims across all live scenes) uses to walk the composed stages.
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = (bevy::asset::AssetId<crate::UsdStageAsset>, &CanonicalStage)> {
        self.by_asset.iter().map(|(id, cs)| (*id, cs))
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

    /// Force-rebuild the live stage for `asset` from `recipe`, **replacing** any
    /// existing one (and its sink). Used when an async asset reload lands (twin
    /// overlay edits) and the reconcile must read the *post-edit* composed stage:
    /// rebuilding inline here — from the reloaded asset's own fresh recipe — makes
    /// the reconcile self-contained, with no ordering dependency on the separate
    /// [`sync_canonical_stages`] system that reacts to the same asset event.
    /// Returns `false` if the build fails.
    pub fn rebuild(
        &mut self,
        asset: bevy::asset::AssetId<crate::UsdStageAsset>,
        recipe: &crate::StageRecipe,
    ) -> bool {
        match CanonicalStage::from_recipe(recipe) {
            Ok(cs) => {
                self.by_asset.insert(asset, cs);
                true
            }
            Err(e) => {
                bevy::log::warn!("[canonical] rebuild from recipe failed for {asset:?}: {e}");
                false
            }
        }
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

// Temp-dir USDA fixtures, native-only test code. The `std::fs` ban guards wasm
// *runtime* paths; `clippy.toml` names tests as exempt, but cargo has no
// path-scoped lint config, so the exemption is written out.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::disallowed_methods)]
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
#[allow(clippy::disallowed_methods)] // temp-dir USDA fixtures; see `recipe_tests`
mod sync_system_tests {
    //! Ph0′ S2d-wiring: the `sync_canonical_stages` SYSTEM, in a minimal Bevy
    //! App, must turn a loaded `UsdStageAsset` (carrying a `StageRecipe`) into a
    //! live `CanonicalStage` in the `CanonicalStages` resource — the exact runtime
    //! path the headless server exercises on scene load.

    use super::*;
    use bevy::asset::{AssetApp, AssetPlugin};
    use bevy::prelude::*;

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

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<crate::UsdStageAsset>()
            .init_non_send::<CanonicalStages>()
            .add_systems(Update, sync_canonical_stages);

        // `Assets::add` emits `AssetEvent::Added`, which the system reads.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<crate::UsdStageAsset>>()
            .add(crate::UsdStageAsset { recipe: Some(recipe) });

        // One frame flushes the asset event; the next lets the system act on it.
        app.update();
        app.update();

        let stages = app
            .world()
            .get_non_send::<CanonicalStages>()
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
mod authoring_tests {
    //! Keystone (write half): authoring op-deltas onto the live `CanonicalStage`
    //! fires the change sink, so the projection bridge reconciles the edit — the
    //! "author onto the stage → sink → project" loop, headless.

    use super::*;
    use crate::UsdRead;

    const SCENE: &str =
        "#usda 1.0\n(\n    defaultPrim = \"World\"\n)\ndef Xform \"World\"\n{\n    def Xform \"Rover\"\n    {\n    }\n}\n";

    fn touches(changes: &[RawStageChange], path: &str) -> bool {
        changes.iter().any(|c| {
            c.info_only.iter().chain(c.resynced.iter()).any(|p| p.to_string() == path)
        })
    }

    #[test]
    fn authoring_translate_prim_remove_fires_sink() {
        let recipe = StageRecipe::from_source("scene.usda", SCENE);
        let mut cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let _ = cs.drain_changes(); // clear any initial notices

        // MOVE: author a translate → sink reports the prim; it composes live.
        let rover = SdfPath::new("/World/Rover").unwrap();
        cs.author_translate(&rover, [1.0, 2.0, 3.0]).expect("author translate");
        assert!(touches(&cs.drain_changes(), "/World/Rover"), "translate fires the sink");
        assert_eq!(
            crate::read_vec3_f64(&cs.view(), &rover, "xformOp:translate"),
            Some([1.0, 2.0, 3.0]),
            "the authored translate composes on the live stage"
        );

        // SPAWN (plain): define a prim → resync; it's live.
        let r2 = SdfPath::new("/World/Rover2").unwrap();
        cs.author_prim(&r2, Some("Xform")).expect("author prim");
        assert!(
            cs.drain_changes().iter().any(|c| c.resynced.iter().any(|p| p.to_string() == "/World/Rover2")),
            "defining a prim fires a resync"
        );
        assert!(cs.view().has_prim(&r2), "the defined prim is live on the stage");

        // REMOVE: drop it → resync; it's gone.
        assert!(cs.remove_prim_at(&r2).expect("remove prim"));
        assert!(
            cs.drain_changes().iter().any(|c| c.resynced.iter().any(|p| p.to_string() == "/World/Rover2")),
            "removing a prim fires a resync"
        );
        assert!(!cs.view().has_prim(&r2), "the removed prim is gone from the stage");
    }

    /// A keyframe authored onto the live stage fires the sink and composes as a
    /// `timeSamples` opinion — the write half of incremental keyframe projection.
    /// The per-frame animation sampler reads this stage, so no whole-scene rebuild
    /// is needed for a key on an already-animated prim.
    #[test]
    fn authoring_time_sample_fires_sink_and_composes() {
        let recipe = StageRecipe::from_source("scene.usda", SCENE);
        let mut cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        let _ = cs.drain_changes();

        let rover = SdfPath::new("/World/Rover").unwrap();
        let v = crate::author::parse_attribute_value("double3", "(1, 2, 3)").unwrap();
        cs.author_time_sample(&rover, "xformOp:translate", "double3", 12.0, v)
            .expect("author keyframe");
        assert!(touches(&cs.drain_changes(), "/World/Rover"), "keyframe fires the sink");
        assert!(
            cs.view().has_time_samples(&rover, "xformOp:translate"),
            "the authored keyframe composes as timeSamples on the live stage"
        );
    }

    /// Keystone of #1 — **referenced spawn onto a live stage**: inject an asset's
    /// layer bytes at runtime, author a prim + a `references` arc to it, and PCP
    /// composes the referenced subtree on the live stage (its default prim's
    /// children land under the spawn) with the sink reporting the resync — no
    /// whole-scene rebuild, no async reload. This is what lets the palette-spawn
    /// of a rover ride the "author onto the stage → sink → project" loop.
    #[test]
    fn injected_reference_spawns_composed_subtree_and_fires_sink() {
        // The scene knows NOTHING about the rover — it isn't in the recipe.
        let recipe = StageRecipe::from_source("scene.usda", SCENE);
        let mut cs = CanonicalStage::from_recipe(&recipe).expect("build scene stage");
        let _ = cs.drain_changes();

        // The rover asset (with a defaultPrim so a bare reference targets it).
        const ROVER: &str = "#usda 1.0\n(\n    defaultPrim = \"RoverRoot\"\n)\ndef Xform \"RoverRoot\"\n{\n    def Cube \"Body\"\n    {\n    }\n}\n";

        // The reference id PCP will demand === what we inject the bytes under.
        let asset_path = "rover.usda";
        let ref_id = cs.canonical_reference_id(asset_path);
        assert!(!cs.has_layer_bytes(&ref_id), "rover not loaded yet");

        // Inject the rover's closure into the LIVE resolver, then author the
        // spawn: a prim + a reference to the rover.
        assert!(
            cs.add_layer_bytes(HashMap::from([(ref_id.clone(), ROVER.as_bytes().to_vec())])),
            "a recipe-built stage must accept injected layer bytes"
        );
        assert!(cs.has_layer_bytes(&ref_id), "bytes now present in the live resolver");

        let spawn = SdfPath::new("/World/rover_1").unwrap();
        cs.author_prim(&spawn, Some("Xform")).expect("define the spawn prim");
        cs.author_reference(&spawn, asset_path).expect("author the reference arc");

        // The sink reports the spawn path as resynced (the projector reconciles it).
        assert!(
            cs.drain_changes()
                .iter()
                .any(|c| c.resynced.iter().any(|p| p.to_string() == "/World/rover_1")),
            "authoring a referenced spawn must resync its prim"
        );

        // And PCP composed the referenced subtree onto the live stage: the
        // rover's `Body` child is now present under the spawn.
        let body = SdfPath::new("/World/rover_1/Body").unwrap();
        assert!(
            cs.view().has_prim(&body),
            "the referenced rover's Body child must compose under the runtime spawn"
        );
    }

    // ── Object-builder live authors (doc 48 §3.2/§3.3) ──
    // These four are what made the rebuild cliff go away: each edit now composes on
    // the LIVE stage instead of forcing a whole-scene rebuild. The document-level
    // `UsdOp` tests prove the ops author into save-data; THESE prove the live-stage
    // counterparts compose, which is the claim the projector arms actually depend on.

    const RIG: &str = "#usda 1.0\n(\n    defaultPrim = \"Rig\"\n)\ndef Xform \"Rig\"\n{\n    def Xform \"Chassis\"\n    {\n    }\n    def Xform \"Wheel\"\n    {\n    }\n    def PhysicsRevoluteJoint \"Hinge\"\n    {\n    }\n    def Xform \"Bus\"\n    {\n        float inputs:voltage\n    }\n    def Xform \"Battery\"\n    {\n        float outputs:voltage = 28\n    }\n}\n";

    #[test]
    fn author_relationship_composes_joint_bodies_on_live_stage() {
        let recipe = StageRecipe::from_source("rig.usda", RIG);
        let mut cs = CanonicalStage::from_recipe(&recipe).expect("build rig");
        let _ = cs.drain_changes();

        let hinge = SdfPath::new("/Rig/Hinge").unwrap();
        cs.author_relationship(&hinge, "physics:body0", &["/Rig/Chassis".into()])
            .expect("author body0");
        cs.author_relationship(&hinge, "physics:body1", &["/Rig/Wheel".into()])
            .expect("author body1");

        // The joint's two bodies compose on the LIVE stage — this is the read the
        // Avian joint builder does. Before the live author, this required a rebuild.
        assert_eq!(
            cs.view().rel_targets(&hinge, "physics:body0").iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            vec!["/Rig/Chassis".to_string()],
        );
        assert_eq!(
            cs.view().rel_targets(&hinge, "physics:body1").iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            vec!["/Rig/Wheel".to_string()],
        );
    }

    #[test]
    fn author_connection_composes_on_live_stage() {
        // The silent-bug fix: SetConnection was classified incremental but had no
        // live author, so a wire drawn at runtime vanished until the next rebuild.
        let recipe = StageRecipe::from_source("rig.usda", RIG);
        let mut cs = CanonicalStage::from_recipe(&recipe).expect("build rig");
        let _ = cs.drain_changes();

        let bus = SdfPath::new("/Rig/Bus").unwrap();
        cs.author_connection(&bus, "inputs:voltage", "float", &["/Rig/Battery.outputs:voltage".into()])
            .expect("author connection");

        assert_eq!(
            cs.view().connections(&bus, "inputs:voltage"),
            vec!["/Rig/Battery.outputs:voltage".to_string()],
            "the authored wire composes on the live stage (was silently dropped before)"
        );
    }

    // SetApiSchemas / SetActive have no live-stage author here on purpose: their
    // ECS effect (physics component set / entity presence) can't be reconciled by
    // the visual-only subtree refresh, so they take the projector's rebuild path.
    // Their document-level authoring + inverse are covered in
    // `lunco_usd::document::tests`, and their rebuild routing in
    // `lunco_usd::twin_projection::tests`.
}
