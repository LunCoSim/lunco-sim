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
        let mut catalog = Self { entries: Vec::new() };

        // Built-in entries that aren't plain USD files (procedural factories)
        // or that want a hand-tuned spawn lift live here. Everything authored
        // as a `*.usda` (rovers, components, Twin structures) is discovered
        // dynamically by `populate_dynamic_spawn_catalog` — dropping a file in
        // an open Twin makes it spawnable with no rebuild. `add_unique` lets a
        // hand-tuned entry below win over its discovered twin (same id).

        // --- Rovers (USD; listed here only to pin a 1 m spawn lift) ---
        catalog.add(SpawnableEntry {
            id: "skid_rover".into(),
            display_name: "Skid Rover".into(),
            category: "Rovers".into(),
            source: SpawnSource::UsdFile("vessels/rovers/skid_rover.usda".into()),
            spawn_lift: 1.0,
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "ackermann_rover".into(),
            display_name: "Ackermann Rover".into(),
            category: "Rovers".into(),
            source: SpawnSource::UsdFile("vessels/rovers/ackermann_rover.usda".into()),
            spawn_lift: 1.0,
            default_transform: Transform::default(),
        });

        // --- Procedural props/terrain (no USD file to discover) ---
        catalog.add(SpawnableEntry {
            id: "ball_dynamic".into(),
            display_name: "Dynamic Ball".into(),
            category: "Props".into(),
            source: SpawnSource::Procedural(ProceduralId::BallDynamic),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "ball_static".into(),
            display_name: "Static Ball".into(),
            category: "Props".into(),
            source: SpawnSource::Procedural(ProceduralId::BallStatic),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "ramp".into(),
            display_name: "Ramp".into(),
            category: "Terrain".into(),
            source: SpawnSource::Procedural(ProceduralId::Ramp),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "wall".into(),
            display_name: "Wall".into(),
            category: "Terrain".into(),
            source: SpawnSource::Procedural(ProceduralId::Wall),
            spawn_lift: 0.0,
            default_transform: Transform::default(),
        });

        catalog
    }
}

impl SpawnCatalog {
    fn add(&mut self, entry: SpawnableEntry) {
        self.entries.push(entry);
    }

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
    /// Load from a USD file via the asset server.
    UsdFile(String),
    /// Spawned procedurally by a factory function.
    Procedural(ProceduralId),
}

/// Identifier for procedural spawn types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProceduralId {
    /// Dynamic sphere (RigidBody + Collider + Mesh).
    BallDynamic,
    /// Static sphere (Collider + Mesh, no RigidBody).
    BallStatic,
    /// Angled cuboid ramp (static RigidBody).
    Ramp,
    /// Tall cuboid wall (static RigidBody).
    Wall,
}

/// Result of spawning an entry. Contains the root entity/entities created.
pub struct SpawnResult {
    /// The root entity of the spawned object.
    pub root_entity: Entity,
}

/// Spawns a procedural entry at the given world position.
pub fn spawn_procedural(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    entry: &SpawnableEntry,
    world_pos: Vec3,
    grid: Entity,
) -> SpawnResult {
    let root = match entry.source {
        SpawnSource::Procedural(ProceduralId::BallDynamic) => {
            let radius = 0.5_f32;
            let mesh = meshes.add(Sphere::new(radius).mesh().ico(16).unwrap());
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.3, 0.3),
                ..default()
            });
            commands.spawn((
                Name::new("Dynamic Ball"),
                lunco_core::SelectableRoot,
                Transform::from_translation(world_pos),
                avian3d::prelude::RigidBody::Dynamic,
                avian3d::prelude::Collider::sphere(radius as f64),
                avian3d::prelude::Mass(5.0),
                avian3d::prelude::Friction::new(0.5),
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                ChildOf(grid),
            )).id()
        }
        SpawnSource::Procedural(ProceduralId::BallStatic) => {
            let radius = 0.5_f32;
            let mesh = meshes.add(Sphere::new(radius).mesh().ico(16).unwrap());
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.3, 0.3, 0.9),
                ..default()
            });
            commands.spawn((
                Name::new("Static Ball"),
                lunco_core::SelectableRoot,
                Transform::from_translation(world_pos),
                avian3d::prelude::Collider::sphere(radius as f64),
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                ChildOf(grid),
            )).id()
        }
        SpawnSource::Procedural(ProceduralId::Ramp) => {
            let (w, h, d) = (6.0_f64, 2.0_f64, 8.0_f64);
            let mesh = meshes.add(Cuboid::new(w as f32, h as f32, d as f32));
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.5, 0.5, 0.5),
                ..default()
            });
            commands.spawn((
                Name::new("Ramp"),
                lunco_core::SelectableRoot,
                Transform::from_translation(world_pos)
                    .with_rotation(Quat::from_rotation_z(17.1887_f32.to_radians())),
                avian3d::prelude::RigidBody::Static,
                avian3d::prelude::Collider::cuboid(w, h, d),
                avian3d::prelude::Friction::new(1.0),
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                ChildOf(grid),
            )).id()
        }
        SpawnSource::Procedural(ProceduralId::Wall) => {
            let (w, h, d) = (8.0_f64, 4.0_f64, 1.0_f64);
            let mesh = meshes.add(Cuboid::new(w as f32, h as f32, d as f32));
            let mat = materials.add(StandardMaterial {
                base_color: Color::srgb(0.6, 0.6, 0.6),
                ..default()
            });
            commands.spawn((
                Name::new("Wall"),
                lunco_core::SelectableRoot,
                Transform::from_translation(world_pos),
                avian3d::prelude::RigidBody::Static,
                avian3d::prelude::Collider::cuboid(w, h, d),
                Mesh3d(mesh),
                MeshMaterial3d(mat),
                ChildOf(grid),
            )).id()
        }
        _ => panic!("Unknown procedural spawn: {:?}", entry.source),
    };

    SpawnResult { root_entity: root }
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
    if let SpawnSource::UsdFile(ref path) = entry.source {
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

        return SpawnResult { root_entity: root };
    }
    panic!("spawn_usd_entry called with non-USD source");
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
        // Scenes/missions are whole worlds, not spawnable parts.
        if a.rel.contains("scenes/") || a.rel.contains("missions/") {
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
    fn test_catalog_has_builtin_entries() {
        // Only procedural + spawn-lift-pinned entries are hardcoded now;
        // USD-file entries (components, structures) arrive via dynamic scan.
        let catalog = SpawnCatalog::default();
        assert!(catalog.by_category("Rovers").count() >= 2);
        assert!(catalog.by_category("Props").count() >= 2);
        assert!(catalog.by_category("Terrain").count() >= 2);
        // Distinct, sorted category labels for dynamic UI grouping.
        assert!(catalog.categories().contains(&"Rovers".to_string()));
    }

    #[test]
    fn test_catalog_get_by_id() {
        let catalog = SpawnCatalog::default();
        assert!(catalog.get("skid_rover").is_some());
        assert!(catalog.get("ackermann_rover").is_some());
        assert!(catalog.get("ball_dynamic").is_some());
        assert!(catalog.get("ramp").is_some());
        assert!(catalog.get("wall").is_some());
        assert!(catalog.get("nonexistent").is_none());
        // Spawn lift is per-entry data, not a category rule.
        assert_eq!(catalog.get("skid_rover").unwrap().spawn_lift, 1.0);
    }
}
