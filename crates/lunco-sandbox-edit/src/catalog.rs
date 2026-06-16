//! Spawn catalog — registry of all spawnable object types.
//!
//! The catalog is built at startup and contains entries for rovers, props,
//! and terrain. Each entry knows how to spawn itself (USD file or procedural).

use bevy::prelude::*;
use lunco_usd_bevy::UsdPrimPath;

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
    grid: Entity,
) -> SpawnResult {
    let SpawnSource::UsdFile(path) = &entry.source;
    let handle = asset_server.load(path.clone());

    // Derive USD prim path from entry id: "solar_panel" → "/SolarPanel"
    let prim_path = entry.id.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().chain(chars).collect(),
            }
        })
        .collect::<Vec<_>>()
        .join("");
    let prim_path = format!("/{}", prim_path);

    let root = commands.spawn((
        Name::new(entry.display_name.clone()),
        lunco_core::SelectableRoot,
        lunco_core::GridAnchor,
        UsdPrimPath {
            stage_handle: handle,
            path: prim_path,
        },
        Transform::from_translation(world_pos),
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
/// right default for structures authored with origin at the ground plane.
/// This is the "spawn height described in USD, dynamic" path.
fn read_spawn_lift(path: &std::path::Path) -> f32 {
    let Ok(src) = std::fs::read_to_string(path) else { return 0.0 };
    for line in src.lines() {
        if let Some(rest) = line.split_once("lunco:spawnLift") {
            // `float lunco:spawnLift = 2.0`
            if let Some(v) = rest.1.split('=').nth(1) {
                if let Ok(f) = v.trim().parse::<f32>() {
                    return f;
                }
            }
        }
    }
    0.0
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

/// Populate the catalog with USD assets discovered project-wide
/// (`lunco_assets::discovery::list_usd_assets` — the DRY single source of
/// truth, scanning the engine library + every open Twin). Idempotent via
/// `add_unique`, so hand-tuned built-ins win and re-runs add only new files.
/// Re-runs whenever the set of open Twins changes (so dropping a `.usda` into
/// a freshly-opened Twin makes it spawnable with no rebuild). On wasm the
/// discovery list is empty (no filesystem), so this is a cheap no-op there.
pub fn populate_dynamic_spawn_catalog(
    twin_roots: Option<Res<lunco_assets::twin_source::TwinRoots>>,
    mut catalog: ResMut<SpawnCatalog>,
    mut last_twins: Local<Vec<String>>,
    mut did_engine_scan: Local<bool>,
) {
    let Some(roots) = twin_roots.as_deref() else { return };
    let names = roots.names();
    // Engine library is static — scan it once; a changed twin set re-scans.
    if *did_engine_scan && names == *last_twins {
        return;
    }
    *did_engine_scan = true;
    *last_twins = names;

    let added = scan_usd_into_catalog(roots, &mut catalog);
    if added > 0 {
        info!("SPAWN_CATALOG: +{added} USD asset(s) discovered");
    }
}

/// Add every project USD asset (engine library + open Twins) to `catalog`,
/// skipping scenes/missions. Idempotent (`add_unique`). Returns the count
/// newly added. Shared by the auto-scan system and the manual rescan command.
pub fn scan_usd_into_catalog(
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut SpawnCatalog,
) -> usize {
    let mut added = 0;
    for a in lunco_assets::discovery::list_usd_assets(roots) {
        // Scenes/missions are whole worlds, not spawnable parts. Catch both a
        // `scenes/`/`missions/` folder and a root-level `*_scene.usda`
        // (e.g. the Twin's `moonbase_scene.usda`).
        if a.rel.contains("scenes/")
            || a.rel.contains("missions/")
            || a.stem.ends_with("_scene")
        {
            continue;
        }
        if catalog.add_unique(SpawnableEntry {
            id: a.stem.clone(),
            display_name: title_case(&a.stem),
            category: categorize(&a.rel),
            source: SpawnSource::UsdFile(a.asset_path.clone()),
            spawn_lift: read_spawn_lift(&a.abs_path),
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
}
