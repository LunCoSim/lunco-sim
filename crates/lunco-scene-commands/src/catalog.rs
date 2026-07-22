//! Spawn catalog — the registry of everything spawnable, derived from the USD.
//!
//! Nothing here is hardcoded. A spawnable is any project `*.usda` that says it is
//! one (`bool lunco:spawnable`), its palette group is its folder, and its drop
//! height is its own `float lunco:spawnLift`. Drop a file into `assets/` or an
//! open Twin and it is spawnable, with no Rust change and no rebuild.
//!
//! # The scan is asynchronous, and has to be
//!
//! Two questions, two different costs:
//!
//! - *Which files exist?* — [`lunco_assets::discovery`], synchronous. The native
//!   build walks the directory; the web build reads a manifest baked at build
//!   time, because HTTP has no `readdir` and a bundle's contents genuinely ARE a
//!   build-time fact.
//! - *What does a file say about itself?* — requires **reading it**, which on the
//!   web is an HTTP fetch. That is not a build-time fact: it is the content of a
//!   file we ship, and it can be read from the file we ship.
//!
//! The web build used to answer the second question from a `build.rs` table
//! (`BAKED_SPAWN_META`, `BAKED_DESCRIPTIONS`) — the assets' contents *copied into
//! the binary*, along with the weaker line-scan parser needed to produce them (a
//! build script cannot use the crate's own USD stack). A copy that could go stale
//! against the very files it described, and a second parser that had already
//! drifted from the real one.
//!
//! So the read is async now, on both platforms, and the parse is openusd's. See
//! [`crate::spawn_meta`] for the full account, and [`lunco_assets::asset_read`]
//! for the bytes.
//!
//! The shape is dispatch/drain: [`dispatch_usd_scan`] starts one read per
//! newly-discovered asset (Startup, and whenever the open-Twin set changes), and
//! [`drain_usd_scan`] folds each result into [`AssetMetaStore`] and, if it is a
//! part, [`SpawnCatalog`].

use bevy::prelude::*;
use lunco_usd_bevy::{UsdInstanceRoot, UsdPrimPath};

/// Marker components used by `lunco-cosim`'s integration tests to tag a
/// test entity that should go through a mini compile-and-wire pipeline
/// mirroring the production translator. Production code spawns balloons
/// from USD (`vessels/balloons/{modelica,python}_balloon.usda`) and
/// `lunco_usd_sim::cosim` handles them end-to-end without these markers.
#[derive(Component, Default)]
pub struct BalloonModelMarker;

/// See [`BalloonModelMarker`] — Python-side test fixture marker.
#[derive(Component, Default)]
pub struct PythonBalloonMarker;

/// Registry of all spawnable object types.
#[derive(Resource)]
pub struct SpawnCatalog {
    pub entries: Vec<SpawnableEntry>,
}

impl Default for SpawnCatalog {
    fn default() -> Self {
        // No hardcoded entries. Everything spawnable is a `*.usda` file
        // (rovers, components, props, Twin structures) discovered at runtime by
        // `populate_dynamic_spawn_catalog` from the project's USD — drop a file
        // in `assets/` or an open Twin and it's spawnable with no rebuild.
        // Per-asset data (category from its folder, `spawn_lift` from a
        // `float lunco:spawnLift` attribute) is read from the USD, so no Rust
        // code is aware of any specific content type.
        Self { entries: Vec::new() }
    }
}

impl SpawnCatalog {
    /// Add `entry` only if no entry with the same `id` exists yet. Returns
    /// `true` if inserted. Used by dynamic discovery so re-scanning is
    /// idempotent and never shadows a hand-tuned built-in entry.
    pub fn add_unique(&mut self, entry: SpawnableEntry) -> bool {
        if self.entries.iter().any(|e| e.id == entry.id) {
            return false;
        }
        self.entries.push(entry);
        true
    }

    /// Get an entry by ID.
    pub fn get(&self, id: &str) -> Option<&SpawnableEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Get all entries in a category (matched by its dynamic string label).
    pub fn by_category<'a>(&'a self, cat: &'a str) -> impl Iterator<Item = &'a SpawnableEntry> {
        self.entries.iter().filter(move |e| e.category == cat)
    }

    /// Distinct category labels present, sorted — drives dynamic UI grouping
    /// so a new content folder yields a new group with no Rust change.
    pub fn categories(&self) -> Vec<String> {
        let mut cats: Vec<String> = self.entries.iter().map(|e| e.category.clone()).collect();
        cats.sort();
        cats.dedup();
        cats
    }
}

/// A single spawnable thing in the catalog.
#[derive(Clone, Debug)]
pub struct SpawnableEntry {
    /// Unique identifier (e.g., "skid_rover", "ball_dynamic").
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Dynamic category label for UI grouping (e.g. "Rovers", "Structures").
    /// Derived from content location, never a hardcoded Rust taxonomy.
    pub category: String,
    /// How this entry is spawned.
    pub source: SpawnSource,
    /// Metres to lift the spawn point above the click/terrain hit. Data, not a
    /// category rule: dynamic props that must drop onto terrain set a positive
    /// value; structures authored with origin at the ground use `0.0`. Sourced
    /// from the USD `float lunco:spawnLift` attribute for discovered assets.
    pub spawn_lift: f32,
    /// Default transform applied at spawn (overridden by click position).
    pub default_transform: Transform,
}

/// How a spawnable entry is created.
#[derive(Clone, Debug)]
pub enum SpawnSource {
    /// Load from a USD file via the asset server. The only spawn source —
    /// every spawnable, including props once built procedurally in Rust, is
    /// now authored as USD and constructed by the USD→Bevy loader.
    UsdFile(String),
}

/// Result of spawning an entry. Contains the root entity/entities created.
pub struct SpawnResult {
    /// The root entity of the spawned object.
    pub root_entity: Entity,
}

/// Derive the USD root prim path from an entry id: snake_case → PascalCase
/// with a leading `/` (e.g. `"skid_rover"` → `"/SkidRover"`,
/// `"rocker_bogie"` → `"/RockerBogie"`). Single home so [`spawn_usd_entry`]
/// and the real-time footprint derivation ([`crate::spawn`]) agree on which
/// prim to walk.
pub fn prim_path_from_entry_id(id: &str) -> String {
    let pascal = id
        .split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join("");
    format!("/{}", pascal)
}

/// The scene root a runtime spawn mounts under — a type, not a bare `Entity`,
/// so a call site cannot pass "some entity" and get a different hierarchy than
/// scene-load produces.
///
/// There is deliberately no second variant. A scene's top-level prims are PLAIN
/// children of the scene root; a body that instead carries its own `CellCoord`
/// under the grid fights avian's `Position`→`Transform` writeback (avian derives
/// the local transform from the parent's `GlobalTransform` and ignores the cell)
/// and its render freezes at the spawn pose while physics keeps integrating.
/// Making "grid-direct" unrepresentable is what stops "spawned" and
/// "scene-loaded" drifting apart again — a caller with no scene root must WAIT
/// for one, not invent another frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpawnAnchor(Entity);

impl SpawnAnchor {
    /// Mount under the scene root. Obtain the entity from a
    /// `Query<Entity, With<UsdSceneRoot>>`; there is no other legal anchor.
    pub fn scene_root(scene_root: Entity) -> Self {
        Self(scene_root)
    }

    /// The entity spawns are parented to.
    pub fn entity(self) -> Entity {
        self.0
    }
}

/// Spawns a USD-based entry at the given world position.
///
/// Returns the root entity that was spawned. The USD asset is loaded
/// asynchronously — the caller should handle the loading state.
///
/// The USD prim path is derived from the entry ID by converting snake_case
/// to PascalCase (e.g., "solar_panel" → "/SolarPanel", "skid_rover" → "/SkidRover").
pub fn spawn_usd_entry(
    commands: &mut Commands,
    asset_server: &AssetServer,
    entry: &SpawnableEntry,
    world_pos: Vec3,
    rotation: Quat,
    anchor: SpawnAnchor,
) -> SpawnResult {
    let SpawnSource::UsdFile(path) = &entry.source;
    let handle = asset_server.load(path.clone());

    let mut ent = commands.spawn((
        Name::new(entry.display_name.clone()),
        lunco_core::SelectableRoot,
        // Seeds hierarchical instance identity (gap G2/B.1): the USD loader
        // gives this runtime spawn's descendants `Derived` ids off this root's
        // unique id, so two spawns of the same asset don't collide. Atomic with
        // `UsdPrimPath` so the spawn observer sees it.
        UsdInstanceRoot,
        UsdPrimPath {
            stage_handle: handle,
            // Empty path = "mount the stage's `defaultPrim`" sentinel (resolved
            // by the loader, which writes the concrete path back — see
            // `instantiate_usd_prim` in lunco-usd-bevy). USD is the source of
            // truth for the root prim; deriving `/PascalCase(stem)` from the
            // filename silently mounts a non-existent prim (→ invisible spawn)
            // whenever the file stem and its `defaultPrim` disagree (e.g. a
            // `*_glb.usda` wrapper whose prim has no `Glb` suffix).
            path: String::new(),
        },
        Transform {
            translation: world_pos,
            rotation,
            ..default()
        },
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    ));

    // Plain child of the scene root — the same shape scene-load gives a scene's
    // own top-level prims (see [`SpawnAnchor`]). `world_pos` is grid-absolute at
    // cell 0 and the scene root sits at cell 0 / identity, so it is already the
    // correct scene-root-relative local transform.
    ent.insert(ChildOf(anchor.entity()));

    SpawnResult { root_entity: ent.id() }
}

/// Derive a dynamic category label from a discovered asset's path — the name
/// of its immediate parent folder, Title-cased (`structures/habitat.usda` →
/// "Structures", `vessels/rovers/x.usda` → "Rovers"). No hardcoded taxonomy:
/// a new content folder simply becomes a new palette group.
fn categorize(rel: &str) -> String {
    rel.rsplit_once('/')
        .map(|(dir, _)| dir.rsplit('/').next().unwrap_or(dir))
        .map(title_case)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Other".to_string())
}

use crate::spawn_meta::{parse_spawn_meta, SpawnMeta};
use lunco_assets::discovery::AssetFile;

/// What every project `*.usda` says about itself, keyed by its asset path.
///
/// The catalogue's *source*, and the Scenarios menu's tooltip source — one store
/// for one fact. These used to be two: [`SpawnCatalog`] scanned the files for
/// `lunco:spawnable`/`lunco:spawnLift`, and the sandbox UI kept its own
/// `SceneDescCache` that re-parsed the *same default prim of the same files*
/// for `lunco:description`.
///
/// **Eventually complete.** Filled by the async scan below — on the web each
/// entry costs an HTTP fetch, so it lands over some frames rather than all at
/// once. A UI reading it must tolerate a miss (show no tooltip) rather than
/// treat absence as an answer.
#[derive(Resource, Default)]
pub struct AssetMetaStore {
    by_path: std::collections::HashMap<String, SpawnMeta>,
}

impl AssetMetaStore {
    /// This asset's metadata, or `None` if it has not been read yet.
    pub fn get(&self, asset_path: &str) -> Option<&SpawnMeta> {
        self.by_path.get(asset_path)
    }

    /// This asset's `lunco:description` — the "what is this" blurb. `None` when
    /// the asset authors none *or* has not been read yet; both mean "no tooltip".
    pub fn description(&self, asset_path: &str) -> Option<&str> {
        self.by_path.get(asset_path)?.description.as_deref()
    }

    /// How many assets have been read. Only useful for logging/tests.
    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    /// Whether nothing has been read yet.
    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// One asset's bytes, read and parsed.
struct Scanned {
    asset: AssetFile,
    meta: SpawnMeta,
}

/// The in-flight metadata scan.
///
/// Reading an asset is **async** — on the web it is an HTTP fetch, and there is
/// no honest way to make that synchronous. So the scan is a dispatch/drain pair:
/// [`dispatch_usd_scan`] fires one read per newly-discovered asset, and
/// [`drain_usd_scan`] folds the results into [`AssetMetaStore`] + [`SpawnCatalog`]
/// as they land.
#[derive(Resource)]
pub struct CatalogScan {
    tx: crossbeam_channel::Sender<Scanned>,
    rx: crossbeam_channel::Receiver<Scanned>,
    /// Asset paths already dispatched. An asset is read ONCE per rescan — the
    /// scan runs on every Twin-set change, and without this it would re-fetch
    /// the entire engine library each time a twin opened.
    dispatched: std::collections::HashSet<String>,
}

impl Default for CatalogScan {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            dispatched: Default::default(),
        }
    }
}

impl CatalogScan {
    /// Forget what has been read, so the next [`dispatch_usd_scan`] re-reads
    /// every asset. Backs the manual `RescanSpawnCatalog` command — the point of
    /// which is to pick up *edits* to files already seen.
    pub fn forget(&mut self) {
        self.dispatched.clear();
    }
}

/// Read one discovered asset's metadata. The single read path, both platforms:
/// bytes via [`lunco_assets::asset_read`], meaning via openusd.
///
/// An unreadable asset yields [`SpawnMeta::default`] — *not spawnable*. A file we
/// cannot read has not told us it is a part, and guessing "yes" is how a broken
/// asset would end up in the palette.
pub async fn read_asset_meta(asset: &AssetFile) -> SpawnMeta {
    match lunco_assets::asset_read::read_asset_text(asset).await {
        Ok(src) => parse_spawn_meta(&src),
        Err(e) => {
            warn!("CATALOG: {} unreadable, treating as not-spawnable: {e}", asset.rel);
            SpawnMeta::default()
        }
    }
}

/// Fire an async read for every project `*.usda` not yet dispatched. Returns how
/// many reads were started.
///
/// Enumeration is still synchronous — [`lunco_assets::discovery`] answers "what
/// files exist" from the filesystem (native) or the shipped manifest (web).
/// It is only the *contents* that need I/O.
pub fn dispatch_usd_scan(
    manifest: &lunco_assets::discovery::AssetManifest,
    roots: &lunco_assets::twin_source::TwinRoots,
    scan: &mut CatalogScan,
) -> usize {
    let mut started = 0;
    for asset in lunco_assets::discovery::list_usd_assets(manifest, roots) {
        if !scan.dispatched.insert(asset.asset_path.clone()) {
            continue;
        }
        let tx = scan.tx.clone();
        let fut = async move {
            let meta = read_asset_meta(&asset).await;
            // Receiver lives in a resource for the app's lifetime; a send error
            // just means shutdown raced us.
            let _ = tx.send(Scanned { asset, meta });
        };
        // Native: off the main thread. Web: `spawn_local`, because a browser
        // `fetch` future is `!Send` and cannot go on a task pool at all.
        #[cfg(not(target_arch = "wasm32"))]
        bevy::tasks::AsyncComputeTaskPool::get().spawn(fut).detach();
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(fut);
        started += 1;
    }
    started
}

/// Fold completed reads into the metadata store and the spawn catalog. Cheap
/// when idle: an empty channel drains in nothing.
pub fn drain_usd_scan(
    scan: Res<CatalogScan>,
    mut store: ResMut<AssetMetaStore>,
    mut catalog: ResMut<SpawnCatalog>,
) {
    let mut added = 0;
    for Scanned { asset, meta } in scan.rx.try_iter() {
        if meta.spawnable && catalog.add_unique(entry_for(&asset, &meta)) {
            added += 1;
        }
        store.by_path.insert(asset.asset_path, meta);
    }
    if added > 0 {
        info!("CATALOG_SCAN: +{added} spawnable(s)");
    }
}

/// The catalogue entry an asset+metadata pair describes. Pure — no I/O, so the
/// mapping from "what the file says" to "what the palette shows" is testable
/// without touching a disk or a network.
pub fn entry_for(asset: &AssetFile, meta: &SpawnMeta) -> SpawnableEntry {
    SpawnableEntry {
        id: asset.stem.clone(),
        display_name: title_case(&asset.stem),
        category: categorize(&asset.rel),
        source: SpawnSource::UsdFile(asset.asset_path.clone()),
        spawn_lift: meta.lift,
        default_transform: Transform::default(),
    }
}

/// `habitat_fsh` → `Habitat Fsh`. Cheap presentable name from a file stem.
fn title_case(stem: &str) -> String {
    stem.split(['_', '-'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().chain(c).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Enumerate, read and populate in one blocking call — the async pipeline above,
/// collapsed.
///
/// **Native only, and only for tests and one-shot tools.** `block_on` is sound
/// here for the reason [`lunco_storage::Storage::read_sync`] documents: the
/// native backend's future wraps synchronous `std::fs` and is already `Ready`.
/// The browser's is not, which is the whole reason the running app uses the
/// dispatch/drain pair instead.
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_usd_into_catalog_blocking(
    manifest: &lunco_assets::discovery::AssetManifest,
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut SpawnCatalog,
) -> usize {
    let mut added = 0;
    for asset in lunco_assets::discovery::list_usd_assets(manifest, roots) {
        let meta = futures_lite::future::block_on(read_asset_meta(&asset));
        if meta.spawnable && catalog.add_unique(entry_for(&asset, &meta)) {
            added += 1;
        }
    }
    added
}

#[cfg(test)]
mod spawn_anchor_tests {
    use super::*;

    #[derive(Resource)]
    struct SpawnArgs {
        entry: SpawnableEntry,
        scene_root: Entity,
    }

    const POS: Vec3 = Vec3::new(1.0, 2.0, 3.0);

    fn spawn_once(mut commands: Commands, assets: Res<AssetServer>, args: Res<SpawnArgs>) {
        spawn_usd_entry(
            &mut commands,
            &assets,
            &args.entry,
            POS,
            Quat::IDENTITY,
            SpawnAnchor::scene_root(args.scene_root),
        );
    }

    /// Drives the REAL spawn through `Commands` + a flush, then returns the world
    /// so assertions see the anchoring as it actually lands (a bare function call
    /// would prove nothing — the components only exist after the queue applies).
    fn spawn() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(bevy::asset::AssetPlugin::default())
            .init_asset::<lunco_usd_bevy::UsdStageAsset>();

        let scene_root = app.world_mut().spawn(Name::new("Scene:test")).id();

        app.insert_resource(SpawnArgs {
            entry: SpawnableEntry {
                id: "modelica_balloon".into(),
                display_name: "Modelica Balloon".into(),
                category: "Vessels".into(),
                source: SpawnSource::UsdFile("vessels/balloons/modelica_balloon.usda".into()),
                spawn_lift: 0.0,
                default_transform: Transform::default(),
            },
            scene_root,
        });
        app.add_systems(Startup, spawn_once);
        app.update();

        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<UsdInstanceRoot>>();
        let root = q.iter(world).next().expect("spawn produced a root entity");
        (app, root, scene_root)
    }

    /// A runtime spawn must land in the SAME shape scene-load gives a scene's own
    /// top-level prims: a plain child of the `UsdSceneRoot`, carrying no
    /// `CellCoord` of its own. [`SpawnAnchor`] makes the grid-direct shape
    /// unrepresentable; this pins the components that shape actually produces.
    #[test]
    fn spawn_is_a_plain_child_of_the_scene_root_with_no_cell_of_its_own() {
        let (app, root, scene_root) = spawn();
        let world = app.world();

        assert_eq!(
            world.get::<ChildOf>(root).map(|c| c.parent()),
            Some(scene_root),
            "a runtime spawn must parent to the scene-root anchor, not the grid"
        );
        assert!(
            world.get::<big_space::prelude::CellCoord>(root).is_none(),
            "a spawned body must NOT carry its own CellCoord — a grid-direct cell \
             anchor fights avian's Position→Transform writeback and freezes its render"
        );
        assert!(
            world.get::<lunco_core::GridAnchor>(root).is_none(),
            "only the scene-root is the grid anchor; a spawn inherits its frame"
        );
    }

    /// The spawn position is grid-absolute at cell 0 and the scene root sits at
    /// cell 0 / identity, so anchoring must leave the authored coordinate intact.
    #[test]
    fn spawn_preserves_the_requested_coordinate() {
        let (app, root, _scene_root) = spawn();
        assert_eq!(
            app.world().get::<Transform>(root).map(|t| t.translation),
            Some(POS),
            "anchoring must not shift the spawn coordinate"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_case() {
        assert_eq!(title_case("habitat_fsh"), "Habitat Fsh");
        assert_eq!(title_case("solar_tower"), "Solar Tower");
    }

    #[test]
    fn test_categorize_from_folder() {
        assert_eq!(categorize("structures/habitat_fsh.usda"), "Structures");
        assert_eq!(categorize("vessels/rovers/skid_rover.usda"), "Rovers");
        assert_eq!(categorize("components/power/solar_panel.usda"), "Power");
        assert_eq!(categorize("bare.usda"), "Other");
    }

    #[test]
    fn test_add_unique_dedups() {
        let mut c = SpawnCatalog { entries: Vec::new() };
        let mk = |id: &str| SpawnableEntry {
            id: id.into(),
            display_name: id.into(),
            category: "Structures".into(),
            source: SpawnSource::UsdFile("x.usda".into()),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        };
        assert!(c.add_unique(mk("a")));
        assert!(!c.add_unique(mk("a")));
        assert_eq!(c.entries.len(), 1);
    }

    #[test]
    fn test_default_catalog_is_empty() {
        // Nothing hardcoded — every spawnable is discovered from project USD.
        assert!(SpawnCatalog::default().entries.is_empty());
    }

    #[test]
    fn test_categories_distinct_sorted() {
        let mut c = SpawnCatalog { entries: Vec::new() };
        let mk = |id: &str, cat: &str| SpawnableEntry {
            id: id.into(),
            display_name: id.into(),
            category: cat.into(),
            source: SpawnSource::UsdFile("x.usda".into()),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        };
        c.add_unique(mk("a", "Rovers"));
        c.add_unique(mk("b", "Structures"));
        c.add_unique(mk("c", "Rovers"));
        assert_eq!(c.categories(), vec!["Rovers".to_string(), "Structures".to_string()]);
        assert_eq!(c.by_category("Rovers").count(), 2);
    }

    /// Data guard: every shipped sandbox scene must carry a non-empty
    /// `lunco:description` so the Scenarios menu can show a tooltip for it.
    /// A scene missing the attribute would silently show no tooltip — this
    /// test fails loud instead, the moment a scene is added without one.
    ///
    /// Reads the shipped files through the SAME parser the app uses. It used to
    /// go through a `read_usd_description(path)` helper that no longer exists,
    /// because reading a file is now [`lunco_assets::asset_read`]'s job and
    /// understanding it is [`parse_spawn_meta`]'s — this test is about the
    /// *data*, so it does its own read and asserts on the meaning.
    #[test]
    fn test_every_sandbox_scene_has_description() {
        let scenes_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/scenes/sandbox");
        let mut count = 0;
        for e in std::fs::read_dir(&scenes_dir).expect("sandbox scenes dir exists") {
            let p = e.unwrap().path();
            if p.extension().and_then(|s| s.to_str()) != Some("usda") {
                continue;
            }
            count += 1;
            let src = std::fs::read_to_string(&p).expect("scene readable");
            let desc = parse_spawn_meta(&src).description.unwrap_or_else(|| {
                panic!("scene {} has no `lunco:description` attribute", p.display())
            });
            assert!(!desc.trim().is_empty(), "scene {} has an empty description", p.display());
        }
        assert!(count >= 4, "expected the sandbox scene set, found {count}");
    }

    /// The bake this replaced keyed its tables on the engine-relative path and
    /// fell back to "not spawnable" on a miss — so a stale table silently
    /// dropped assets from the web palette. The store is keyed on `asset_path`
    /// (what the catalogue and the UI both hold), and an unread asset is
    /// distinguishable from one that authored nothing.
    #[test]
    fn test_meta_store_absent_vs_authored_nothing() {
        let mut store = AssetMetaStore::default();
        assert!(store.get("scenes/sandbox/x.usda").is_none());
        store.by_path.insert(
            "scenes/sandbox/x.usda".into(),
            SpawnMeta { spawnable: false, lift: 0.0, description: None },
        );
        assert!(store.get("scenes/sandbox/x.usda").is_some());
        assert_eq!(store.description("scenes/sandbox/x.usda"), None);
    }

    /// A rescan must re-read files it has already seen — that is what it is FOR
    /// (picking up an edit). Dispatch is deduped, `forget` clears the dedup.
    #[test]
    fn test_scan_dispatch_dedups_until_forgotten() {
        let mut scan = CatalogScan::default();
        assert!(scan.dispatched.insert("a.usda".into()));
        assert!(!scan.dispatched.insert("a.usda".into()));
        scan.forget();
        assert!(scan.dispatched.insert("a.usda".into()));
    }
}
