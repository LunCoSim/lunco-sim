//! Spawn catalog — the registry of everything spawnable, derived from the USD.
//!
//! Nothing here is hardcoded. A spawnable is any project `*.usda` that says it is
//! one (`bool lunco:spawnable`), and its palette group is its folder. Placement
//! dimensions come from the asset's composed USD collision geometry. Drop a
//! file into `assets/` or an open Twin and it is spawnable, with no Rust change
//! and no rebuild.
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
//! The read is asynchronous on both platforms, and the parse is openusd's. See
//! [`crate::spawn_meta`] for the full account, and [`lunco_assets::asset_read`]
//! for the bytes.
//!
//! The shape is dispatch/drain: [`dispatch_usd_scan`] starts one read per
//! newly-discovered asset (Startup, and whenever the open-Twin set changes), and
//! [`drain_usd_scan`] publishes the completed batch into [`AssetMetaStore`] and,
//! if it is a part, [`SpawnCatalog`].  Publication is batch-atomic: the palette
//! never exposes a half-scanned catalog while the remaining USD files are still
//! being fetched.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_usd_bevy::{UsdInstanceRoot, UsdPrimPath};

/// Registry of all spawnable object types.
#[derive(Resource, Default)]
pub struct SpawnCatalog {
    pub entries: Vec<SpawnableEntry>,
}

/// Structured read of the runtime spawn catalog.
///
/// SpawnEntity accepts an entry_id; scripts and remote clients must be able
/// to discover those ids from the same catalog that the command validates.
/// This provider deliberately reports authored/discovered data only.
pub struct SpawnCatalogProvider;

impl ApiQueryProvider for SpawnCatalogProvider {
    fn name(&self) -> &'static str {
        "ListSpawnCatalog"
    }

    fn execute(&self, world: &World, _params: &serde_json::Value) -> ApiResponse {
        let Some(catalog) = world.get_resource::<SpawnCatalog>() else {
            return ApiResponse::error(
                ApiErrorCode::InternalError,
                "ListSpawnCatalog: SpawnCatalog resource is not present",
            );
        };
        let mut entries: Vec<_> = catalog
            .entries
            .iter()
            .map(|entry| {
                let source = match &entry.source {
                    SpawnSource::UsdFile(path) => serde_json::json!({
                        "kind": "usd_file",
                        "path": path,
                    }),
                };
                serde_json::json!({
                    "entry_id": entry.id,
                    "name": entry.display_name,
                    "category": entry.category,
                    "route_marker": entry.is_route_marker(),
                    "default_transform": {
                        "position": [
                            entry.default_transform.translation.x,
                            entry.default_transform.translation.y,
                            entry.default_transform.translation.z,
                        ],
                        "rotation": [
                            entry.default_transform.rotation.x,
                            entry.default_transform.rotation.y,
                            entry.default_transform.rotation.z,
                            entry.default_transform.rotation.w,
                        ],
                        "scale": [
                            entry.default_transform.scale.x,
                            entry.default_transform.scale.y,
                            entry.default_transform.scale.z,
                        ],
                    },
                    "source": source,
                })
            })
            .collect();
        entries.sort_unstable_by(|a, b| a["entry_id"].as_str().cmp(&b["entry_id"].as_str()));
        let count = entries.len();
        ApiResponse::ok(serde_json::json!({
            "entries": entries,
            "count": count,
        }))
    }
}

/// Register the catalog query beside the catalog resource owner.
pub fn register_query(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(SpawnCatalogProvider);
}

impl SpawnCatalog {
    /// Add `entry` while keeping catalog IDs unique. Re-scanning the same source
    /// is idempotent; two different sources with the same display stem receive a
    /// deterministic source-derived suffix instead of one silently disappearing.
    pub fn add_unique(&mut self, mut entry: SpawnableEntry) -> bool {
        let Some(existing) = self.entries.iter().find(|e| e.id == entry.id) else {
            self.entries.push(entry);
            return true;
        };
        if same_source(existing, &entry) {
            return false;
        }

        let base = entry.id.clone();
        let source_key = match &entry.source {
            SpawnSource::UsdFile(path) => sanitize_id_component(path),
        };
        let mut candidate = format!("{base}__{source_key}");
        let mut suffix = 2;
        loop {
            match self.entries.iter().find(|e| e.id == candidate) {
                None => break,
                Some(existing) if same_source(existing, &entry) => return false,
                Some(_) => {
                    candidate = format!("{base}__{source_key}_{suffix}");
                    suffix += 1;
                }
            }
        }
        entry.id = candidate;
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

fn same_source(a: &SpawnableEntry, b: &SpawnableEntry) -> bool {
    match (&a.source, &b.source) {
        (SpawnSource::UsdFile(a), SpawnSource::UsdFile(b)) => a == b,
    }
}

fn sanitize_id_component(source: &str) -> String {
    let mut component = String::new();
    for c in source.chars() {
        if c.is_ascii_alphanumeric() {
            component.push(c.to_ascii_lowercase());
        } else if !component.ends_with('_') {
            component.push('_');
        }
    }
    component.trim_matches('_').to_string()
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
    /// Default transform applied at spawn (overridden by click position).
    pub default_transform: Transform,
}

impl SpawnableEntry {
    /// Whether this catalog entry is a route member rather than an independent
    /// scene object. Route members need an owning vessel and ordered mission
    /// index, so they must enter through the waypoint route command/tool.
    pub fn is_route_marker(&self) -> bool {
        match &self.source {
            SpawnSource::UsdFile(path) => {
                lunco_assets::engine_asset_rel(path)
                    == lunco_assets::engine_asset_rel(lunco_usd::document::WAYPOINT_MARKER_ASSET)
            }
        }
    }
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

/// The scene root a runtime spawn mounts under — a type, not a bare `Entity`,
/// so a call site cannot pass "some entity" and get a different hierarchy than
/// scene-load produces.
///
/// There is deliberately no second variant. The scene root is itself a nested
/// BigSpace `Grid`, and every spawned top-level prim is a grid-direct child with
/// a `CellCoord`; this keeps runtime and authored scene projections on the same
/// high-precision path. A caller with no scene root must WAIT for one, not
/// invent another frame.
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

/// Spawns a USD-based entry at a grid-direct scene position.
///
/// `cell` and `local_pos` are the storage representation produced by
/// `lunco_core::coords::pose_in_grid_to_parent_storage`; they are not a
/// second semantic coordinate system.
///
/// Returns the root entity that was spawned. The USD asset is loaded
/// asynchronously — the caller should handle the loading state.
///
/// The empty [`UsdPrimPath`] sentinel asks the USD loader to mount the stage's
/// authored `defaultPrim`; the loader writes the resolved path back before the
/// projected subtree is used by runtime consumers.
pub fn spawn_usd_entry(
    commands: &mut Commands,
    asset_server: &AssetServer,
    entry: &SpawnableEntry,
    cell: big_space::prelude::CellCoord,
    local_pos: Vec3,
    rotation: Quat,
    anchor: SpawnAnchor,
) -> SpawnResult {
    let SpawnSource::UsdFile(path) = &entry.source;
    let handle = asset_server.load(path.clone());

    let mut ent = commands.spawn((
        Name::new(entry.display_name.clone()),
        lunco_core::CatalogEntryId(entry.id.clone()),
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
            // truth for the root prim; the asset filename is never used as a
            // path guess.
            path: String::new(),
        },
        Transform {
            translation: local_pos,
            rotation,
            ..default()
        },
        cell,
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    ));

    // Grid-direct child of the scene root — the same shape scene-load gives a
    // scene's own top-level prims (see SpawnAnchor). The command boundary has
    // already converted the semantic pose into this grid's cell/local storage.
    ent.try_insert(ChildOf(anchor.entity()));

    SpawnResult {
        root_entity: ent.id(),
    }
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
/// for one fact. The catalog and Scenarios menu both consume this store, so
/// the same default prim is not parsed again for the standard USD `doc`
/// metadata.
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

    /// This asset's standard USD `doc` metadata — the "what is this" blurb. `None` when
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
    /// Results are staged until the current dispatch batch is complete.
    staged: Vec<Scanned>,
    batch_remaining: usize,
    replace_on_publish: bool,
}

impl Default for CatalogScan {
    fn default() -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self {
            tx,
            rx,
            dispatched: Default::default(),
            staged: Vec::new(),
            batch_remaining: 0,
            replace_on_publish: false,
        }
    }
}

impl CatalogScan {
    /// Forget what has been read, so the next [`dispatch_usd_scan`] re-reads
    /// every asset. Backs the manual `RescanSpawnCatalog` command — the point of
    /// which is to pick up *edits* to files already seen.
    pub fn forget(&mut self) {
        self.dispatched.clear();
        self.replace_on_publish = true;
    }
}

/// Read one discovered asset's metadata. The single read path, both platforms:
/// bytes via [`lunco_assets::asset_read`], meaning via openusd.
///
/// An unreadable asset yields [`SpawnMeta::default`] — *not spawnable*. A file we
/// cannot read has not told us it is a part, and guessing "yes" is how a broken
/// asset would end up in the palette.
pub async fn read_asset_meta(
    asset: &AssetFile,
    settings: &lunco_settings::DownloadSettings,
) -> SpawnMeta {
    match lunco_assets::asset_read::read_asset_text(asset, settings).await {
        Ok(src) => {
            let mut meta = parse_spawn_meta(&src);
            #[cfg(not(target_arch = "wasm32"))]
            if meta.spawnable {
                // The metadata parser answers "did this file opt into the
                // palette?". Native pre-flight answers the next, user-facing
                // question: "will the same file survive the runtime loader?"
                // Keep invalid content out of the palette instead of making a
                // user discover the failure only after dropping it into a sim.
                let report = crate::validate::validate_asset(&asset.abs_path.to_string_lossy());
                if !report.ok {
                    warn!(
                        "CATALOG: {} is marked spawnable but failed load preflight; hiding it from SpawnCatalog: {}",
                        asset.rel,
                        report.errors.join("; ")
                    );
                    meta.spawnable = false;
                }
            }
            meta
        }
        Err(e) => {
            warn!(
                "CATALOG: {} unreadable, treating as not-spawnable: {e}",
                asset.rel
            );
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
    settings: &lunco_settings::DownloadSettings,
) -> usize {
    let mut started = 0;
    let assets = match lunco_assets::discovery::list_usd_assets(manifest, roots) {
        Ok(assets) => assets,
        Err(error) => {
            error!("CATALOG: Twin registry unavailable during USD scan: {error}");
            return 0;
        }
    };
    let settings = settings.clone();
    for asset in assets {
        if !scan.dispatched.insert(asset.asset_path.clone()) {
            continue;
        }
        let tx = scan.tx.clone();
        let settings = settings.clone();
        let fut = async move {
            let meta = read_asset_meta(&asset, &settings).await;
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
    scan.batch_remaining = scan.batch_remaining.saturating_add(started);
    started
}

/// Fold completed reads into the metadata store and the spawn catalog. Cheap
/// when idle: an empty channel drains in nothing.
pub fn drain_usd_scan(
    mut scan: ResMut<CatalogScan>,
    mut store: ResMut<AssetMetaStore>,
    mut catalog: ResMut<SpawnCatalog>,
) {
    let completed: Vec<_> = scan.rx.try_iter().collect();
    for result in completed {
        scan.staged.push(result);
        scan.batch_remaining = scan.batch_remaining.saturating_sub(1);
    }
    if scan.batch_remaining != 0 {
        return;
    }

    if scan.staged.is_empty() {
        if scan.replace_on_publish {
            store.by_path.clear();
            catalog.entries.clear();
            scan.replace_on_publish = false;
        }
        return;
    }

    if scan.replace_on_publish {
        store.by_path.clear();
        catalog.entries.clear();
        scan.replace_on_publish = false;
    }
    // Completion order is nondeterministic because each asset is read by its
    // own async task. Sort the complete batch by canonical source identity
    // before assigning stem-collision IDs; otherwise the first task to finish
    // would win the unsuffixed ID and the same catalog could differ by run.
    // Keep the default library source ahead of named schemes to preserve the
    // catalog's existing unsuffixed stem IDs for shipped assets.
    scan.staged.sort_unstable_by(|a, b| {
        let a_key = (
            lunco_assets::asset_path::split_scheme(&a.asset.asset_path).is_some(),
            &a.asset.asset_path,
        );
        let b_key = (
            lunco_assets::asset_path::split_scheme(&b.asset.asset_path).is_some(),
            &b.asset.asset_path,
        );
        a_key.cmp(&b_key)
    });
    let mut added = 0;
    for Scanned { asset, meta } in scan.staged.drain(..) {
        if meta.spawnable && catalog.add_unique(entry_for(&asset, &meta)) {
            added += 1;
        }
        store.by_path.insert(asset.asset_path, meta);
    }
    if added > 0 {
        info!("CATALOG_SCAN: published batch with +{added} spawnable(s)");
    }
}

/// The catalogue entry an asset+metadata pair describes. Pure — no I/O, so the
/// mapping from "what the file says" to "what the palette shows" is testable
/// without touching a disk or a network.
pub fn entry_for(asset: &AssetFile, _meta: &SpawnMeta) -> SpawnableEntry {
    SpawnableEntry {
        id: asset.stem.clone(),
        display_name: title_case(&asset.stem),
        category: categorize(&asset.rel),
        source: SpawnSource::UsdFile(asset.asset_path.clone()),
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
    settings: &lunco_settings::DownloadSettings,
) -> usize {
    let mut added = 0;
    let assets = match lunco_assets::discovery::list_usd_assets(manifest, roots) {
        Ok(assets) => assets,
        Err(error) => {
            error!("CATALOG: Twin registry unavailable during blocking USD scan: {error}");
            return 0;
        }
    };
    for asset in assets {
        let meta = futures_lite::future::block_on(read_asset_meta(&asset, settings));
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
            big_space::prelude::CellCoord::default(),
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

        let scene_root = app
            .world_mut()
            .spawn((
                Name::new("Scene:test"),
                big_space::prelude::Grid::default(),
                big_space::prelude::CellCoord::default(),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();

        app.insert_resource(SpawnArgs {
            entry: SpawnableEntry {
                id: "modelica_balloon".into(),
                display_name: "Modelica Balloon".into(),
                category: "Vessels".into(),
                source: SpawnSource::UsdFile("vessels/balloons/modelica_balloon.usda".into()),
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
    /// top-level prims: a grid-direct child of the `UsdSceneRoot`, carrying its
    /// own `CellCoord`. [`SpawnAnchor`] keeps the runtime and authored paths on
    /// the same nested scene grid.
    #[test]
    fn spawn_is_a_grid_child_of_the_scene_root_with_its_own_cell() {
        let (app, root, scene_root) = spawn();
        let world = app.world();

        assert_eq!(
            world.get::<ChildOf>(root).map(|c| c.parent()),
            Some(scene_root),
            "a runtime spawn must parent to the scene-root grid"
        );
        assert!(
            world.get::<big_space::prelude::CellCoord>(root).is_some(),
            "a spawned top-level prim must carry a CellCoord in the scene-root grid"
        );
        assert!(
            world.get::<lunco_core::GridAnchor>(root).is_none(),
            "only the scene-root is the grid anchor; a spawn inherits its frame"
        );
    }

    /// The fixture's active frame and scene root coincide, so anchoring must
    /// leave the requested coordinate intact.
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
        let mut c = SpawnCatalog {
            entries: Vec::new(),
        };
        let mk = |id: &str| SpawnableEntry {
            id: id.into(),
            display_name: id.into(),
            category: "Structures".into(),
            source: SpawnSource::UsdFile("x.usda".into()),
            default_transform: Transform::default(),
        };
        assert!(c.add_unique(mk("a")));
        assert!(!c.add_unique(mk("a")));
        assert_eq!(c.entries.len(), 1);
    }

    #[test]
    fn test_add_unique_disambiguates_different_sources() {
        let mut c = SpawnCatalog {
            entries: Vec::new(),
        };
        let entry = |source: &str| SpawnableEntry {
            id: "rover".into(),
            display_name: "Rover".into(),
            category: "Rovers".into(),
            source: SpawnSource::UsdFile(source.into()),
            default_transform: Transform::default(),
        };

        assert!(c.add_unique(entry("vessels/rovers/rover.usda")));
        assert!(c.add_unique(entry("twin://moonbase/vessels/rovers/rover.usda")));
        assert_eq!(c.entries.len(), 2);
        assert_eq!(c.entries[0].id, "rover");
        assert_eq!(
            c.entries[1].id,
            "rover__twin_moonbase_vessels_rovers_rover_usda"
        );
        assert!(!c.add_unique(entry("twin://moonbase/vessels/rovers/rover.usda")));
        assert_eq!(c.entries.len(), 2);
    }

    #[test]
    fn drain_assigns_collision_ids_by_source_order_not_completion_order() {
        let scanned = |source: &str| Scanned {
            asset: AssetFile {
                asset_path: source.into(),
                stem: "rover".into(),
                rel: "vessels/rovers/rover.usda".into(),
                abs_path: source.into(),
                twin: None,
            },
            meta: SpawnMeta {
                spawnable: true,
                description: None,
            },
        };
        let mut scan = CatalogScan::default();
        scan.batch_remaining = 2;
        // Deliberately deliver the Twin result first, as an async completion
        // race would. The drain's source sort must still give the library
        // source the stable unsuffixed ID.
        scan.tx
            .send(scanned("twin://moonbase/vessels/rovers/rover.usda"))
            .unwrap();
        scan.tx.send(scanned("vessels/rovers/rover.usda")).unwrap();

        let mut app = App::new();
        app.insert_resource(scan);
        app.insert_resource(AssetMetaStore::default());
        app.insert_resource(SpawnCatalog::default());
        app.add_systems(Update, drain_usd_scan);
        app.update();

        let catalog = app.world().resource::<SpawnCatalog>();
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.entries[0].id, "rover");
        assert!(matches!(
            &catalog.entries[0].source,
            SpawnSource::UsdFile(path) if path == "vessels/rovers/rover.usda"
        ));
        assert_eq!(
            catalog.entries[1].id,
            "rover__twin_moonbase_vessels_rovers_rover_usda"
        );
    }

    #[test]
    fn test_default_catalog_is_empty() {
        // Nothing hardcoded — every spawnable is discovered from project USD.
        assert!(SpawnCatalog::default().entries.is_empty());
    }

    #[test]
    fn route_marker_is_not_an_independent_spawn_entry() {
        let marker = SpawnableEntry {
            id: "waypoint".into(),
            display_name: "Waypoint".into(),
            category: "Markers".into(),
            source: SpawnSource::UsdFile("vessels/markers/waypoint.usda".into()),
            default_transform: Transform::default(),
        };
        let rover = SpawnableEntry {
            source: SpawnSource::UsdFile("vessels/rovers/ackermann_rover.usda".into()),
            ..marker.clone()
        };

        assert!(marker.is_route_marker());
        assert!(!rover.is_route_marker());
    }

    #[test]
    fn spawn_catalog_provider_exposes_the_command_authority() {
        let mut world = World::new();
        world.insert_resource(SpawnCatalog {
            entries: vec![
                SpawnableEntry {
                    id: "z-last".into(),
                    display_name: "Last".into(),
                    category: "Other".into(),
                    source: SpawnSource::UsdFile("z.usda".into()),
                    default_transform: Transform::from_xyz(1.0, 2.0, 3.0),
                },
                SpawnableEntry {
                    id: "a-first".into(),
                    display_name: "First".into(),
                    category: "Other".into(),
                    source: SpawnSource::UsdFile("a.usda".into()),
                    default_transform: Transform::default(),
                },
            ],
        });

        let response = SpawnCatalogProvider.execute(&mut world, &serde_json::Value::Null);
        let data = match response {
            ApiResponse::Ok { data: Some(data) } => data,
            other => panic!("expected catalog response, got {other:?}"),
        };
        assert_eq!(data["count"], 2);
        assert_eq!(data["entries"][0]["entry_id"], "a-first");
        assert_eq!(data["entries"][1]["entry_id"], "z-last");
        assert_eq!(data["entries"][0]["source"]["kind"], "usd_file");
        assert_eq!(data["entries"][0]["route_marker"], false);
    }

    #[test]
    fn test_categories_distinct_sorted() {
        let mut c = SpawnCatalog {
            entries: Vec::new(),
        };
        let mk = |id: &str, cat: &str| SpawnableEntry {
            id: id.into(),
            display_name: id.into(),
            category: cat.into(),
            source: SpawnSource::UsdFile("x.usda".into()),
            default_transform: Transform::default(),
        };
        c.add_unique(mk("a", "Rovers"));
        c.add_unique(mk("b", "Structures"));
        c.add_unique(mk("c", "Rovers"));
        assert_eq!(
            c.categories(),
            vec!["Rovers".to_string(), "Structures".to_string()]
        );
        assert_eq!(c.by_category("Rovers").count(), 2);
    }

    /// Data guard: every shipped sandbox scene must carry a non-empty standard
    /// USD `doc` metadata field so the Scenarios menu can show a tooltip for it.
    /// A scene missing the attribute would silently show no tooltip — this
    /// test fails loud instead, the moment a scene is added without one.
    ///
    /// Reads the shipped files through the same parser the app uses. Reading
    /// is [`lunco_assets::asset_read`]'s job and understanding the metadata is
    /// [`parse_spawn_meta`]'s; this test asserts the shipped data itself.
    #[test]
    fn test_every_sandbox_scene_has_description() {
        let scenes_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenes/luncosim");
        let mut count = 0;
        for e in std::fs::read_dir(&scenes_dir).expect("sandbox scenes dir exists") {
            let p = e.unwrap().path();
            if p.extension().and_then(|s| s.to_str()) != Some("usda") {
                continue;
            }
            count += 1;
            let src = std::fs::read_to_string(&p).expect("scene readable");
            let desc = parse_spawn_meta(&src)
                .description
                .unwrap_or_else(|| panic!("scene {} has no USD `doc` metadata", p.display()));
            assert!(
                !desc.trim().is_empty(),
                "scene {} has an empty description",
                p.display()
            );
        }
        assert!(count >= 4, "expected the sandbox scene set, found {count}");
    }

    /// Every asset offered by the spawn catalog must explain itself through the
    /// standard USD `doc` metadata on its default prim. Internal component files
    /// may omit it; they are not user-facing spawn options.
    #[test]
    fn every_spawnable_asset_has_description() {
        fn visit(dir: &std::path::Path, missing: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("asset directory exists") {
                let path = entry.expect("asset entry readable").path();
                if path.is_dir() {
                    visit(&path, missing);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("usda") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("USD asset readable");
                let meta = parse_spawn_meta(&source);
                if meta.spawnable
                    && meta
                        .description
                        .as_deref()
                        .is_none_or(|description| description.trim().is_empty())
                {
                    missing.push(path);
                }
            }
        }

        let mut missing = Vec::new();
        visit(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets"),
            &mut missing,
        );
        assert!(
            missing.is_empty(),
            "spawnable USD assets need default-prim `doc` metadata: {missing:?}"
        );
    }

    /// The bake this replaced keyed its tables on the engine-relative path and
    /// fell back to "not spawnable" on a miss — so a stale table silently
    /// dropped assets from the web palette. The store is keyed on `asset_path`
    /// (what the catalogue and the UI both hold), and an unread asset is
    /// distinguishable from one that authored nothing.
    #[test]
    fn test_meta_store_absent_vs_authored_nothing() {
        let mut store = AssetMetaStore::default();
        assert!(store.get("scenes/luncosim/x.usda").is_none());
        store.by_path.insert(
            "scenes/luncosim/x.usda".into(),
            SpawnMeta {
                spawnable: false,
                description: None,
            },
        );
        assert!(store.get("scenes/luncosim/x.usda").is_some());
        assert_eq!(store.description("scenes/luncosim/x.usda"), None);
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

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn read_asset_meta_hides_spawnable_assets_that_fail_load_preflight() {
        let dir = std::env::temp_dir().join("lunco-spawn-catalog-preflight");
        std::fs::create_dir_all(&dir).expect("temporary preflight directory");
        let path = dir.join("broken_wheel.usda");
        std::fs::write(
            &path,
            "#usda 1.0\n( defaultPrim = \"BrokenWheel\" )\n\
def Xform \"BrokenWheel\" (\n\
    prepend apiSchemas = [\"LunCoCatalogAPI\", \"PhysxVehicleWheelAPI\"]\n\
)\n{\n    uniform bool lunco:spawnable = true\n}\n",
        )
        .expect("write malformed spawnable fixture");

        let asset = AssetFile {
            asset_path: path.to_string_lossy().into_owned(),
            stem: "broken_wheel".into(),
            rel: "broken_wheel.usda".into(),
            abs_path: path,
            twin: None,
        };
        let meta = futures_lite::future::block_on(read_asset_meta(
            &asset,
            &lunco_settings::DownloadSettings::default(),
        ));

        assert!(
            !meta.spawnable,
            "an asset that the runtime loader rejects must not reach SpawnCatalog"
        );
    }
}
