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
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use big_space::prelude::CellCoord;
use lunco_core::{on_command, register_commands, Command, GridAnchor, WorldGrid};
use lunco_obstacle_field::field::{HeightGrid, MeshData};

use crate::bake::{crop_centered, resample};
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

/// The driveable DEM surface: the bake fills **this** entity with a heightfield
/// collider (+ visual mesh when rendering). Put on a command-spawned entity by
/// [`SpawnDemTerrain`], or on a USD terrain prim by the USD→DEM bridge so the
/// universal `materialType="shader"` path supplies the material.
#[derive(Component)]
pub struct DemTerrainSurface;

/// A request to build a DEM tile **onto the entity carrying this component**.
/// [`start_dem_builds`] kicks the off-thread bake; [`finish_dem_builds`] inserts
/// `Mesh3d` + `Collider` onto the same entity. Public so the USD→DEM bridge (in
/// `lunco-sandbox`) can place it on an authored terrain prim.
#[derive(Component)]
pub struct DemTerrainRequest {
    /// DEM site directory (contains `metadata.yaml` + `materials/textures/heightmap.tif`).
    pub uri: String,
    /// Half side length (metres) of the centred region to realize at native
    /// resolution. `f64::INFINITY` = the whole DEM.
    pub half_window: f64,
    /// Visual-quality knob: if `> 0` and below the cropped native resolution, the
    /// tile is **resampled** (lossy downsample, via [`crate::bake::resample`]) to
    /// this many samples per side before meshing — so you can A/B different DEM
    /// qualities (256² … native) on the same site. `0` = keep native (default).
    /// NOTE: this coarsens the **whole** tile (mesh + collider together); the M7
    /// streaming ring is what keeps the near-field collider native while only the
    /// far visual LOD decimates.
    pub target_res: usize,
    /// Suppress the static visual mesh and instead stream camera-driven CDLOD
    /// tiles (procedural-regolith geomorph; see [`crate::stream_viz`]). The
    /// heightfield COLLIDER still spawns, so physics is unchanged. This is the
    /// production visual path (default ON from the USD bridge); `false` = the
    /// single static mesh.
    pub lod_viz: bool,
    /// Opt-in: stream a per-rover canonical-res heightfield collider ring instead
    /// of one static full-DEM collider (see [`crate::collider_ring`]). When `true`
    /// the static collider is **suppressed** (the ring replaces it — overlapping
    /// heightfields would double up contacts). `false` = the single static collider.
    pub collider_ring: bool,
    /// Apply a plain `StandardMaterial` when the bake finishes. `true` for the
    /// standalone command path; `false` for the USD path, where the prim's
    /// `materialType` authors the material (don't clobber it).
    pub with_default_material: bool,
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
    /// Visual-quality downsample target (samples per side). `0` = native (no
    /// decimation). Re-issue the command with a different value to rebuild the
    /// same site at another quality and compare.
    pub target_res: u32,
    /// Stream camera-driven CDLOD tiles (procedural-regolith geomorph) instead of
    /// one static mesh; collider/physics unchanged. Production visual path.
    pub lod_viz: bool,
    /// Stream a per-rover canonical-res collider ring instead of one static
    /// full-DEM collider (replaces it — physics rides the streamed tiles).
    pub collider_ring: bool,
}

#[on_command(SpawnDemTerrain)]
fn on_spawn_dem_terrain(
    trigger: On<SpawnDemTerrain>,
    grids: Query<Entity, With<WorldGrid>>,
    mut commands: Commands,
) {
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
    // Standalone entity, anchored into the world grid at the origin cell (when it
    // exists). The USD path instead places `DemTerrainRequest` on the prim entity,
    // which already carries its USD transform + grid parentage.
    let mut e = commands.spawn((
        DemTerrainSurface,
        Name::new("DemTerrain"),
        DemTerrainRequest {
            uri: ev.uri.clone(),
            half_window,
            target_res: ev.target_res as usize,
            lod_viz: ev.lod_viz,
            collider_ring: ev.collider_ring,
            with_default_material: true,
        },
        Transform::IDENTITY,
        Visibility::Inherited,
    ));
    if let Ok(grid) = grids.single() {
        e.insert((CellCoord::default(), GridAnchor, ChildOf(grid)));
    }
    info!(
        "[dem-terrain] queued build for {} (window {} m, target_res {})",
        ev.uri, ev.window_m, ev.target_res
    );
}

register_commands!(on_spawn_dem_terrain);

/// What the off-thread build produces, ready to assemble into entities.
struct DemBuild {
    /// The static visual mesh — `None` in `lod_viz` mode (tiles draw instead).
    mesh: Option<MeshData>,
    collider_heights: Vec<Vec<f64>>,
    /// The realized tile grid, always retained as the shared `HeightSource` the
    /// streaming consumers and the `TerrainHeight` query sample.
    grid: Option<std::sync::Arc<HeightGrid>>,
    half_extent: f32,
    /// Tile resolution actually meshed (= native crop res, or the resample target).
    res: usize,
    /// Native crop resolution before any resample (for honest logging).
    native_res: usize,
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
        let target_res = req.target_res;
        let lod_viz = req.lod_viz;
        let collider_ring = req.collider_ring;

        let task = AsyncComputeTaskPool::get().spawn(async move {
            let meta_bytes = read_bytes(meta_path).await?;
            let meta_str = String::from_utf8(meta_bytes)
                .map_err(|e| format!("metadata.yaml not utf-8: {e}"))?;
            let meta = DemMetadata::from_yaml_str(&meta_str).map_err(|e| e.to_string())?;

            let tif = read_bytes(tif_path).await?;
            let grid = height_grid_from_geotiff(&tif, &meta).map_err(|e| e.to_string())?;

            // Crop the playable region at native resolution. The mesh and collider
            // share this grid so visuals and contact agree.
            let tile = crop_centered(&grid, half_window);
            let native_res = tile.res;
            // Optional visual-quality downsample (lossy). 0 / ≥ native keeps native.
            let tile = if target_res > 0 && target_res < native_res {
                resample(&tile, tile.half_extent as f64, target_res)
            } else {
                tile
            };
            let half_extent = tile.half_extent;
            let res = tile.res;
            // The static collider is built from these heights; skip the (whole-DEM)
            // allocation when the collider ring will stream per-tile colliders instead.
            let collider_heights = if collider_ring { Vec::new() } else { tile.to_avian_heights() };
            // Static visual mesh unless lod_viz streams visual tiles instead.
            let mesh = if lod_viz { None } else { Some(tile.to_mesh_data()) };
            // Always retain the grid as a shared `HeightSource`. The streaming
            // consumers (visual `lod_viz`, physics `collider_ring`) sample it, and
            // so does the `TerrainHeight` API/scripting query — which must work on
            // the plain static terrain too (see `crate::query`). Retaining is free:
            // `to_avian_heights`/`to_mesh_data` only borrow, and the `Arc` is shared.
            let grid = Some(std::sync::Arc::new(tile));
            Ok(DemBuild {
                collider_heights,
                mesh,
                grid,
                half_extent,
                res,
                native_res,
                site: meta.site_id,
            })
        });
        commands.entity(entity).insert(DemBuildTask(task));
    }
}

/// Collect finished builds and fill the requesting entity with the heightfield
/// collider (+ visual mesh when rendering).
fn finish_dem_builds(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut DemBuildTask, &DemTerrainRequest)>,
    // Optional so the headless server (no render assets) still builds colliders.
    meshes: Option<ResMut<Assets<Mesh>>>,
    materials: Option<ResMut<Assets<StandardMaterial>>>,
) {
    use bevy::tasks::futures_lite::future;

    let mut meshes = meshes;
    let mut materials = materials;

    for (entity, mut task, req) in &mut tasks {
        let Some(result) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        // Request consumed — drop the task + request marker either way.
        commands.entity(entity).remove::<(DemBuildTask, DemTerrainRequest)>();

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
        let mut e = commands.entity(entity);
        // Static full-DEM collider — UNLESS the collider ring replaces it with
        // streamed per-tile colliders (overlapping heightfields would double up).
        if !req.collider_ring {
            let collider =
                Collider::heightfield(built.collider_heights, DVec3::new(2.0 * h, 1.0, 2.0 * h));
            e.insert((RigidBody::Static, collider));
        }
        // Retain the grid + mark the streaming mode(s). `lod_viz` streams visual LOD
        // tiles (static mesh suppressed); `collider_ring` streams physics tiles
        // (static collider suppressed above). Both sample the retained `DemHeightField`.
        if let Some(grid) = built.grid {
            e.insert(crate::stream_viz::DemHeightField(grid));
            if req.lod_viz {
                e.insert((
                    crate::stream_viz::TerrainLodViz::default(),
                    crate::stream_viz::LodTiles::default(),
                    // Default Lit; switchable live in the Inspector (Terrain Shader).
                    crate::stream_viz::TerrainShaderMode::default(),
                ));
            }
            if req.collider_ring {
                e.insert((
                    crate::collider_ring::TerrainColliderRing::default(),
                    crate::collider_ring::ColliderTiles::default(),
                ));
            }
        }
        if let (Some(meshes), Some(mesh)) = (meshes.as_mut(), built.mesh) {
            let MeshData { positions, normals, uvs, indices } = mesh;
            e.insert(Mesh3d(meshes.add(lunco_obstacle_field::grid_mesh(
                positions, normals, uvs, indices,
            ))));
            // Default material only for the standalone command path; the USD path
            // authors its own via `materialType` (don't clobber it).
            if req.with_default_material {
                if let Some(materials) = materials.as_mut() {
                    e.insert(MeshMaterial3d(materials.add(StandardMaterial {
                        base_color: Color::srgb(0.30, 0.29, 0.27),
                        perceptual_roughness: 1.0,
                        ..default()
                    })));
                }
            }
        }
        let mode = match (req.lod_viz, req.collider_ring) {
            (true, true) => " [lod-viz + collider-ring]",
            (true, false) => " [lod-viz: streaming tiles]",
            (false, true) => " [collider-ring: streaming colliders]",
            (false, false) => "",
        };
        if built.res == built.native_res {
            info!("[dem-terrain] built '{}' ({}² native, ±{:.0} m){}", built.site, built.res, h, mode);
        } else {
            info!(
                "[dem-terrain] built '{}' ({}² resampled from {}² native, ±{:.0} m){}",
                built.site, built.res, built.native_res, h, mode
            );
        }
    }
}

/// Register the DEM-terrain command + spawn systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.register_type::<SpawnDemTerrain>()
        .add_systems(Update, (start_dem_builds, finish_dem_builds));
    register_all_commands(app);
}
