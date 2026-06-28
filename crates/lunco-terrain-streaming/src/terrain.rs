//! M3: spawn a static terrain entity from a real DEM asset.
//!
//! Fire the [`SpawnDemTerrain`] command (`uri` = a `lunar_terrain_exporter` site
//! directory). The DEM bytes are read through `lunco-storage` (cross-platform),
//! decoded + resampled **off the main thread**, then a single static entity is
//! spawned with an avian `Collider::heightfield` (always) and a Bevy mesh (when
//! render assets exist — the headless server builds colliders only, so physics
//! stays identical server/client). Anchored into the big_space world grid at the
//! origin cell, mirroring `lunco-obstacle-field`.
//!
//! This is the non-streamed spine: one downsampled tile of the whole DEM. Tiled
//! streaming + LOD + a per-rover canonical-res collider ring come later (M7); the
//! `resample` bridge and this spawn path are what they build on.

use avian3d::prelude::{Collider, RigidBody};
use bevy::asset::RenderAssetUsages;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_mesh::Indices;
use big_space::prelude::CellCoord;
use lunco_core::{on_command, register_commands, Command, GridAnchor, WorldGrid};
use lunco_obstacle_field::field::{HeightGrid, MeshData};

use crate::bake::crop_centered;
use crate::dem::{height_grid_from_geotiff, DemMetadata};

/// Default realized region side length (metres) when `window_m` is 0… no — see
/// below: 0 means the whole map. This is the fallback when a caller passes a
/// negative value. A 4 km window at 5 m is 800² ≈ 640 k verts — full detail,
/// light to render, and covers a rover working area.
const DEFAULT_WINDOW_M: f32 = 4096.0;
/// Above this native tile resolution we still build, but warn: a single mesh this
/// large is heavy (e.g. the full 16 km map is 3200² ≈ 10 M verts ≈ 560 MB). Full
/// detail at that scale belongs to tiled streaming (M7), not one mesh.
const HEAVY_TILE_RES: usize = 2048;

/// Root of a spawned DEM terrain (anchored into the world grid).
#[derive(Component)]
pub struct DemTerrainRoot;

/// The driveable DEM surface (heightfield collider + optional visual mesh).
#[derive(Component)]
pub struct DemTerrainSurface;

/// Pending request entity: carries the build parameters until the async task is
/// attached.
#[derive(Component)]
struct DemTerrainRequest {
    uri: String,
    /// Half side length (metres) of the centred region to realize at native
    /// resolution. `f64::INFINITY` = the whole DEM.
    half_window: f64,
}

/// Build a DEM terrain from a site directory at **native resolution**. `uri`
/// points at a `lunar_terrain_exporter` output dir (containing `metadata.yaml`
/// and `materials/textures/heightmap.tif`).
///
/// `window_m` is the side length (metres) of the centred region realized as one
/// full-5 m-resolution tile (mesh + collider). `0` = the whole DEM (heavy — a
/// 16 km map is ~10 M verts; prefer tiled streaming). Detail is **never**
/// decimated.
#[Command(default)]
pub struct SpawnDemTerrain {
    pub uri: String,
    pub window_m: f32,
}

#[on_command(SpawnDemTerrain)]
fn on_spawn_dem_terrain(trigger: On<SpawnDemTerrain>, mut commands: Commands) {
    let ev = trigger.event();
    if ev.uri.is_empty() {
        warn!("[dem-terrain] SpawnDemTerrain with empty uri ignored");
        return;
    }
    let half_window = match ev.window_m {
        w if w == 0.0 => f64::INFINITY,          // whole map
        w if w < 0.0 => (DEFAULT_WINDOW_M * 0.5) as f64,
        w => (w * 0.5) as f64,
    };
    commands.spawn(DemTerrainRequest { uri: ev.uri.clone(), half_window });
    info!("[dem-terrain] queued build for {} (window {} m)", ev.uri, ev.window_m);
}

register_commands!(on_spawn_dem_terrain);

/// What the off-thread build produces, ready to assemble into entities.
struct DemBuild {
    mesh: MeshData,
    collider_heights: Vec<Vec<f64>>,
    half_extent: f32,
    /// Native tile resolution actually realized (for logging / heavy-mesh warning).
    res: usize,
    site: String,
}

#[derive(Component)]
struct DemBuildTask(Task<Result<DemBuild, String>>);

/// Read a file's bytes through the cross-platform storage abstraction.
///
/// Native uses `FileStorage`; the wasm arm is an explicit "not yet" at the I/O
/// boundary only — the decode/resample/spawn above are platform-agnostic, and
/// the web strategy is pre-baked streamed tiles (M7), not fetching a 40 MB
/// monolithic DEM. So this is **not** a native-gated feature: it compiles and
/// runs on wasm and fails with a clear message rather than silently vanishing.
async fn read_bytes(path: std::path::PathBuf) -> Result<Vec<u8>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use lunco_storage::{FileStorage, Storage, StorageHandle};
        FileStorage::new()
            .read(&StorageHandle::File(path))
            .await
            .map_err(|e| e.to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err("web DEM byte source not yet wired (planned with tiled streaming, M7); \
             the decode/resample path is platform-agnostic"
            .to_string())
    }
}

/// Kick an off-thread build for each pending request.
fn start_dem_builds(
    mut commands: Commands,
    q: Query<(Entity, &DemTerrainRequest), Without<DemBuildTask>>,
) {
    for (entity, req) in &q {
        let dir = std::path::PathBuf::from(&req.uri);
        let meta_path = dir.join("metadata.yaml");
        let tif_path = dir.join("materials/textures/heightmap.tif");
        let half_window = req.half_window;

        let task = AsyncComputeTaskPool::get().spawn(async move {
            let meta_bytes = read_bytes(meta_path).await?;
            let meta_str = String::from_utf8(meta_bytes)
                .map_err(|e| format!("metadata.yaml not utf-8: {e}"))?;
            let meta = DemMetadata::from_yaml_str(&meta_str).map_err(|e| e.to_string())?;

            let tif = read_bytes(tif_path).await?;
            let grid = height_grid_from_geotiff(&tif, &meta).map_err(|e| e.to_string())?;

            // Native resolution — crop the playable region, never decimate. The
            // mesh and collider share this grid so visuals and contact agree.
            let tile = crop_centered(&grid, half_window);
            Ok(DemBuild {
                collider_heights: tile.to_avian_heights(),
                mesh: tile.to_mesh_data(),
                half_extent: tile.half_extent,
                res: tile.res,
                site: meta.site_id,
            })
        });
        commands.entity(entity).insert(DemBuildTask(task));
    }
}

/// Build a Bevy mesh from raw height-grid vertex data (same shape as
/// `lunco-obstacle-field`'s terrain mesh).
fn terrain_mesh(data: MeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, data.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, data.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, data.uvs);
    mesh.insert_indices(Indices::U32(data.indices));
    mesh
}

/// Collect finished builds and spawn the terrain entity.
fn finish_dem_builds(
    mut commands: Commands,
    grids: Query<Entity, With<WorldGrid>>,
    mut tasks: Query<(Entity, &mut DemBuildTask)>,
    // Optional so the headless server (no render assets) still builds colliders.
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    use bevy::tasks::futures_lite::future;

    if tasks.is_empty() {
        return;
    }
    // Defer until the big_space world grid exists.
    let Ok(grid_entity) = grids.single() else {
        return;
    };

    let mut meshes = meshes;
    let mut materials = materials;

    for (request_entity, mut task) in &mut tasks {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(request_entity).despawn(); // consume the request

        let built = match result {
            Ok(b) => b,
            Err(err) => {
                warn!("[dem-terrain] build failed: {err}");
                continue;
            }
        };

        if built.res > HEAVY_TILE_RES {
            warn!(
                "[dem-terrain] '{}' tile is {}² verts — heavy for a single mesh; \
                 tiled streaming + LOD (M7) is the path for full-map detail",
                built.site, built.res,
            );
        }

        let h = built.half_extent as f64;
        let collider =
            Collider::heightfield(built.collider_heights, DVec3::new(2.0 * h, 1.0, 2.0 * h));

        let root = commands
            .spawn((
                DemTerrainRoot,
                Name::new(format!("DemTerrain/{}", built.site)),
                CellCoord::default(),
                GridAnchor,
                ChildOf(grid_entity),
                Transform::IDENTITY,
                Visibility::Inherited,
            ))
            .id();

        let mut surface = commands.spawn((
            DemTerrainSurface,
            Name::new("DemTerrain/Surface"),
            ChildOf(root),
            Transform::IDENTITY,
            RigidBody::Static,
            collider,
        ));
        if let (Some(meshes), Some(materials)) = (meshes.as_mut(), materials.as_mut()) {
            let mesh = meshes.add(terrain_mesh(built.mesh));
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.30, 0.29, 0.27),
                perceptual_roughness: 1.0,
                ..default()
            });
            surface.insert((Mesh3d(mesh), MeshMaterial3d(material)));
        }
        info!("[dem-terrain] spawned '{}' ({}² native, ±{:.0} m)", built.site, built.res, h);
    }
}

/// Register the DEM-terrain command + spawn systems. Called from
/// [`crate::plugin::TerrainStreamingPlugin`].
pub(crate) fn register(app: &mut App) {
    app.register_type::<SpawnDemTerrain>()
        .add_systems(Update, (start_dem_builds, finish_dem_builds));
    register_all_commands(app);
}
