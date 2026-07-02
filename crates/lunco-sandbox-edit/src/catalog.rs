//! Spawn catalog — registry of all spawnable object types.
//!
//! The catalog is built at startup and contains entries for rovers, props,
//! and terrain. Each entry knows how to spawn itself (USD file or procedural).

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
    grid: Entity,
) -> SpawnResult {
    let SpawnSource::UsdFile(path) = &entry.source;
    let handle = asset_server.load(path.clone());

    let root = commands.spawn((
        Name::new(entry.display_name.clone()),
        lunco_core::SelectableRoot,
        lunco_core::GridAnchor,
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
        big_space::prelude::CellCoord::default(),
        ChildOf(grid),
        Visibility::Visible,
        InheritedVisibility::VISIBLE,
        ViewVisibility::default(),
    )).id();

    SpawnResult { root_entity: root }
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

/// Read the optional `float lunco:spawnLift` attribute from a USD file by a
/// cheap line scan (no full parse). Returns `0.0` if absent/unreadable — the
/// Spawn-related metadata read from a USD file by a cheap line scan (no full
/// parse). Both fields are authored on the default prim:
/// - `float lunco:spawnLift` — metres to lift the spawn point (default `0.0`).
/// - `bool  lunco:spawnable` — whether this file is a spawnable part at all
///   (default `true`); scenes set it `false` so they're not offered as
///   instances. Data-driven, so no Rust code special-cases "scenes".
struct SpawnMeta {
    lift: f32,
    spawnable: bool,
}

#[cfg(not(target_arch = "wasm32"))]
fn read_spawn_meta(path: &std::path::Path) -> SpawnMeta {
    let mut meta = SpawnMeta { lift: 0.0, spawnable: true };
    let Ok(src) = std::fs::read_to_string(path) else { return meta };
    for line in src.lines() {
        if let Some((_, rhs)) = line.split_once("lunco:spawnLift") {
            if let Some(v) = rhs.split('=').nth(1) {
                if let Ok(f) = v.trim().parse::<f32>() {
                    meta.lift = f;
                }
            }
        } else if let Some((_, rhs)) = line.split_once("lunco:spawnable") {
            if let Some(v) = rhs.split('=').nth(1) {
                meta.spawnable = v.trim().starts_with("true") || v.trim().starts_with('1');
            }
        }
    }
    meta
}

/// Spawn metadata baked by `build.rs` (the browser can't line-scan the USD
/// files). Keyed by engine-relative path.
#[cfg(target_arch = "wasm32")]
mod baked_spawn_meta {
    include!(concat!(env!("OUT_DIR"), "/baked_spawn_meta.rs"));
}

/// Web: look the spawn metadata up in the baked manifest. `path` is the bare
/// engine-relative path (`discovery::list_assets` sets `abs_path` to it on
/// wasm), matching the keys `build.rs` baked. Unknown ⇒ spawnable default.
#[cfg(target_arch = "wasm32")]
fn read_spawn_meta(path: &std::path::Path) -> SpawnMeta {
    let key = path.to_str().unwrap_or_default();
    for (rel, spawnable, lift) in baked_spawn_meta::BAKED_SPAWN_META {
        if *rel == key {
            return SpawnMeta { lift: *lift, spawnable: *spawnable };
        }
    }
    SpawnMeta { lift: 0.0, spawnable: true }
}

/// Read a USD scene's `lunco:description` attribute — the human-readable
/// "what is this demo" line shown as a tooltip in the Scenarios menu. Returns
/// `None` if absent/unreadable, in which case the menu just shows no tooltip.
///
/// Native: parses the `.usda` source with **openusd's USDA parser**
/// ([`lunco_usd_bevy::read_default_prim_attr`]) and reads the attribute off the
/// stage's `defaultPrim` through the real USD data model — not a hand-rolled
/// text scan, so string escapes and quoted runs are handled correctly. Only
/// the single layer is parsed (no PCP composition): the description lives on
/// the root prim, so no referenced sub-layer needs resolving.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_usd_description(path: &std::path::Path) -> Option<String> {
    let Ok(src) = std::fs::read_to_string(path) else { return None };
    lunco_usd_bevy::read_default_prim_attr(&src, "lunco:description")
}

/// Descriptions baked by `build.rs` (the browser has no filesystem to parse at
/// runtime). Keyed by engine-relative path — the same string
/// `discovery::list_assets` bakes as `asset_path`, so the wasm lookup matches.
#[cfg(target_arch = "wasm32")]
mod baked_descriptions {
    include!(concat!(env!("OUT_DIR"), "/baked_descriptions.rs"));
}

/// Web: look the description up in the baked manifest. `path` is the bare
/// engine-relative path (`discovery::list_assets` sets `abs_path` to it on
/// wasm), matching the keys `build.rs` baked. Unknown ⇒ no tooltip. The bake
/// is produced by `build.rs` parsing the same `lunco:description` attribute,
/// so the web value matches the native openusd read.
#[cfg(target_arch = "wasm32")]
pub fn read_usd_description(path: &std::path::Path) -> Option<String> {
    let key = path.to_str().unwrap_or_default();
    baked_descriptions::BAKED_DESCRIPTIONS
        .iter()
        .find(|(rel, _)| *rel == key)
        .map(|(_, d)| d.to_string())
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

/// Add every project USD asset (engine library + open Twins) to `catalog`,
/// reading each file's `lunco:spawnable`/`lunco:spawnLift` metadata. A file
/// with `lunco:spawnable = false` (scenes/missions opt out) is skipped — the
/// decision is USD data, not a Rust filename rule. Idempotent (`add_unique`),
/// so re-runs add only new files. Returns the count newly added. Called at
/// Startup and on Twin-open (see `commands.rs`) — never per frame.
pub fn scan_usd_into_catalog(
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut SpawnCatalog,
) -> usize {
    let mut added = 0;
    for a in lunco_assets::discovery::list_usd_assets(roots) {
        let meta = read_spawn_meta(&a.abs_path);
        if !meta.spawnable {
            continue;
        }
        if catalog.add_unique(SpawnableEntry {
            id: a.stem.clone(),
            display_name: title_case(&a.stem),
            category: categorize(&a.rel),
            source: SpawnSource::UsdFile(a.asset_path.clone()),
            spawn_lift: meta.lift,
            default_transform: Transform::default(),
        }) {
            added += 1;
        }
    }
    added
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

    #[test]
    fn test_read_usd_description_from_temp_file() {
        // Round-trips a authored `lunco:description` through the real openusd
        // parser (via `read_usd_description`'s file→parse→read path).
        let dir = std::env::temp_dir();
        let path = dir.join("lunco_catalog_desc_test.usda");
        std::fs::write(
            &path,
            "#usda 1.0\n\
             (defaultPrim = \"X\")\n\
             def Xform \"X\"\n{\n\
                custom string lunco:description = \"A plain-language scene blurb.\"\n\
             }\n",
        )
        .unwrap();
        assert_eq!(
            read_usd_description(&path).as_deref(),
            Some("A plain-language scene blurb.")
        );
    }

    #[test]
    fn test_read_usd_description_none_when_absent() {
        let dir = std::env::temp_dir();
        let path = dir.join("lunco_catalog_desc_none_test.usda");
        std::fs::write(
            &path,
            "#usda 1.0\n\
             (defaultPrim = \"X\")\n\
             def Xform \"X\"\n{\n\
                custom bool lunco:spawnable = false\n\
             }\n",
        )
        .unwrap();
        assert!(read_usd_description(&path).is_none());
    }

    /// Data guard: every shipped sandbox scene must carry a non-empty
    /// `lunco:description` so the Scenarios menu can show a tooltip for it.
    /// A scene missing the attribute would silently show no tooltip — this
    /// test fails loud instead, the moment a scene is added without one.
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
            let desc = read_usd_description(&p).unwrap_or_else(|| {
                panic!(
                    "scene {} has no `lunco:description` attribute",
                    p.display()
                )
            });
            assert!(!desc.trim().is_empty(), "scene {} has an empty description", p.display());
        }
        assert!(count >= 14, "expected the sandbox scene set, found {count}");
    }
}
