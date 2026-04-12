//! Spawn catalog — registry of all spawnable object types.
//!
//! The catalog is built at startup and contains entries for rovers, props,
//! and terrain. Each entry knows how to spawn itself (USD file or procedural).

use bevy::prelude::*;
use lunco_usd_bevy::UsdPrimPath;

/// Registry of all spawnable object types.
#[derive(Resource)]
pub struct SpawnCatalog {
    pub entries: Vec<SpawnableEntry>,
}

impl Default for SpawnCatalog {
    fn default() -> Self {
        let mut catalog = Self { entries: Vec::new() };

        // --- Rovers ---
        catalog.add(SpawnableEntry {
            id: "skid_rover".into(),
            display_name: "Skid Rover".into(),
            category: SpawnCategory::Rover,
            source: SpawnSource::UsdFile("vessels/rovers/skid_rover.usda".into()),
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "ackermann_rover".into(),
            display_name: "Ackermann Rover".into(),
            category: SpawnCategory::Rover,
            source: SpawnSource::UsdFile("vessels/rovers/ackermann_rover.usda".into()),
            default_transform: Transform::default(),
        });

        // --- Components ---
        catalog.add(SpawnableEntry {
            id: "solar_panel".into(),
            display_name: "Solar Panel".into(),
            category: SpawnCategory::Component,
            source: SpawnSource::UsdFile("components/power/solar_panel.usda".into()),
            default_transform: Transform::from_xyz(0.0, 3.0, 0.0),  // 3m above ground
        });

        // --- Props ---
        catalog.add(SpawnableEntry {
            id: "ball_dynamic".into(),
            display_name: "Dynamic Ball".into(),
            category: SpawnCategory::Prop,
            source: SpawnSource::Procedural(ProceduralId::BallDynamic),
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "ball_static".into(),
            display_name: "Static Ball".into(),
            category: SpawnCategory::Prop,
            source: SpawnSource::Procedural(ProceduralId::BallStatic),
            default_transform: Transform::default(),
        });

        // --- Terrain ---
        catalog.add(SpawnableEntry {
            id: "ramp".into(),
            display_name: "Ramp".into(),
            category: SpawnCategory::Terrain,
            source: SpawnSource::Procedural(ProceduralId::Ramp),
            default_transform: Transform::default(),
        });
        catalog.add(SpawnableEntry {
            id: "wall".into(),
            display_name: "Wall".into(),
            category: SpawnCategory::Terrain,
            source: SpawnSource::Procedural(ProceduralId::Wall),
            default_transform: Transform::default(),
        });

        catalog
    }
}

impl SpawnCatalog {
    fn add(&mut self, entry: SpawnableEntry) {
        self.entries.push(entry);
    }

    /// Get an entry by ID.
    pub fn get(&self, id: &str) -> Option<&SpawnableEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Get all entries in a category.
    pub fn by_category(&self, cat: SpawnCategory) -> impl Iterator<Item = &SpawnableEntry> {
        self.entries.iter().filter(move |e| e.category == cat)
    }
}

/// A single spawnable thing in the catalog.
#[derive(Clone, Debug)]
pub struct SpawnableEntry {
    /// Unique identifier (e.g., "skid_rover", "ball_dynamic").
    pub id: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Category for UI grouping.
    pub category: SpawnCategory,
    /// How this entry is spawned.
    pub source: SpawnSource,
    /// Default transform applied at spawn (overridden by click position).
    pub default_transform: Transform,
}

/// Category for organizing the spawn palette UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpawnCategory {
    Rover,
    Component,
    Prop,
    Terrain,
}

impl std::fmt::Display for SpawnCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnCategory::Rover => write!(f, "Rovers"),
            SpawnCategory::Component => write!(f, "Components"),
            SpawnCategory::Prop => write!(f, "Props"),
            SpawnCategory::Terrain => write!(f, "Terrain"),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_has_entries() {
        let catalog = SpawnCatalog::default();
        assert!(!catalog.entries.is_empty());

        // Verify categories exist
        assert!(catalog.by_category(SpawnCategory::Rover).count() >= 2);
        assert!(catalog.by_category(SpawnCategory::Component).count() >= 1);
        assert!(catalog.by_category(SpawnCategory::Prop).count() >= 2);
        assert!(catalog.by_category(SpawnCategory::Terrain).count() >= 2);
    }

    #[test]
    fn test_catalog_get_by_id() {
        let catalog = SpawnCatalog::default();
        assert!(catalog.get("skid_rover").is_some());
        assert!(catalog.get("ackermann_rover").is_some());
        assert!(catalog.get("solar_panel").is_some());
        assert!(catalog.get("ball_dynamic").is_some());
        assert!(catalog.get("ramp").is_some());
        assert!(catalog.get("wall").is_some());
        assert!(catalog.get("nonexistent").is_none());
    }

    #[test]
    fn test_spawn_category_display() {
        assert_eq!(format!("{}", SpawnCategory::Rover), "Rovers");
        assert_eq!(format!("{}", SpawnCategory::Component), "Components");
        assert_eq!(format!("{}", SpawnCategory::Prop), "Props");
        assert_eq!(format!("{}", SpawnCategory::Terrain), "Terrain");
    }
}
