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
use lunco_obstacle_field::sampler::{salt, sample_layer};
use lunco_obstacle_field::spec::{CraterLayer, Pattern};

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

/// Stamp the shared [`CraterLayer`] (from the global [`ObstacleFieldSpec`]) into a
/// DEM working grid as REAL geometry — so the large basins appear in BOTH the
/// streamed visual mesh AND the heightfield collider (you can drive into them).
///
/// **Non-destructive** (the source DEM bytes are never touched — only the in-memory
/// working copy) and **deterministic** (the seed drives placement, so every
/// networked peer regenerates identical basins with nothing to transfer; the spec
/// itself already replicates). Distinct from the `terrain_geomorph` shader's fine
/// normal-only crater texture — this layer owns the big, drivable basins.
///
/// Craters fill the WHOLE DEM window (`grid.half_extent`), not the spec's
/// `region_half_extent` (which bounds the near-field rock scatter). Placement is
/// **blue-noise** with a `min_spacing` derived from crater size + density — NOT the
/// spec's `pattern` (which tunes rocks): `stamp_crater` is *additive*, so uniform
/// overlap stacks rims into spike artifacts and sums bowls into bottomless holes,
/// and a 3 m Poisson spacing over an 8 km window would blow up. Returns the count
/// stamped. Pure → safe to call off-thread.
pub(crate) fn stamp_spec_craters(grid: &mut HeightGrid, craters: &CraterLayer, seed: u64) -> usize {
    let placements = crater_placements(craters, seed, grid.half_extent);
    grid.stamp_craters(&placements, craters);
    placements.len()
}

/// The deterministic crater placements for a terrain of the given `half_extent` —
/// the SAME set [`stamp_spec_craters`] rasterises into the grid, so the dedicated
/// high-fidelity crater mesh (the craters layer's overlay) lands exactly on the
/// stamped basins. Blue-noise `min_spacing` derived from crater size + density (NOT
/// the spec pattern): `stamp_crater` is additive, so uniform overlap stacks rims into
/// spikes, and a 3 m Poisson over an 8 km window would blow up.
pub(crate) fn crater_placements(
    craters: &CraterLayer,
    seed: u64,
    half_extent: f32,
) -> Vec<lunco_obstacle_field::sampler::Placement> {
    if !craters.enabled || craters.density <= 0.0 {
        return Vec::new();
    }
    let side = (2.0 * half_extent) as f64;
    let count = ((craters.density as f64 * side * side) / 10_000.0).round().max(0.0) as usize;
    if count == 0 {
        return Vec::new();
    }
    let pitch = (side / count.max(1) as f64).sqrt() as f32 * 0.7;
    let min_spacing = (craters.size.mode * 2.0).max(pitch).max(6.0);
    sample_layer(
        seed,
        salt::CRATERS,
        Pattern::PoissonDisk { min_spacing },
        half_extent,
        count,
        craters.size,
        0.0,
    )
}

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
    /// **Intelligent upscaling** factor for the working grid. The DEM is coarse
    /// (~5 m); generated craters are *procedural* and can be far crisper than that.
    /// `> 1` bilinearly **upscales the coarse ground** to a finer working grid
    /// (no fake ground detail — just smoother interpolation) BEFORE the crater layer
    /// stamps into it, so craters get high fidelity (sub-5 m rims) decoupled from the
    /// DEM resolution. `1` = native. Cost: the static collider + height grid grow
    /// ~factor² (a load-time structural choice; crater *shape* stays live-tunable).
    pub detail_upsample: usize,
}

/// Retained on a built DEM terrain so its crater layer can be **re-baked live**
/// (Inspector → `RegenerateField`) without re-reading the GeoTIFF: the cropped /
/// resampled grid BEFORE any craters were stamped. [`crate::derived_layers`]'s
/// regenerate path clones this, re-stamps the current [`ObstacleFieldSpec`]
/// craters, and swaps the result into [`crate::stream_viz::DemHeightField`].
#[derive(Component, Clone)]
pub struct DemBaseGrid(pub std::sync::Arc<HeightGrid>);

/// Retained build settings a live re-bake needs (whether the static collider or a
/// streamed collider ring carries physics).
#[derive(Component, Clone, Copy)]
pub struct DemTerrainSource {
    pub collider_ring: bool,
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
    /// Convenience: add a crater layer at this density (craters per hectare). `0`
    /// (default) = no craters. The USD path instead composes layers as child prims
    /// (see [`crate::terrain_layers`]); this is for the quick command path.
    pub crater_density: f32,
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
    // Compose the layer stack: just a crater layer when requested. (The USD path
    // composes richer stacks from child layer prims.)
    let mut stack = crate::terrain_layers::TerrainLayerStack::default();
    if ev.crater_density > 0.0 {
        stack.0.push(crate::terrain_layers::make_crater_layer(ev.crater_density, 22.0, 0.3, 0xC0FFEE));
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
            detail_upsample: 1,
        },
        stack,
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
    /// The realized tile grid (with craters stamped), always retained as the shared
    /// `HeightSource` the streaming consumers and the `TerrainHeight` query sample.
    grid: Option<std::sync::Arc<HeightGrid>>,
    /// The same tile grid BEFORE craters were stamped — retained so a live
    /// `RegenerateField` re-bakes craters off it without re-reading the GeoTIFF.
    base_grid: std::sync::Arc<HeightGrid>,
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
    q: Query<
        (Entity, &DemTerrainRequest, Option<&crate::terrain_layers::TerrainLayerStack>),
        Without<DemBuildTask>,
    >,
) {
    for (entity, req, stack) in &q {
        let dir = std::path::PathBuf::from(&req.uri);
        let meta_path = dir.join("metadata.yaml");
        let tif_path = dir.join("materials/textures/heightmap.tif");
        let half_window = req.half_window;
        let target_res = req.target_res;
        let lod_viz = req.lod_viz;
        let collider_ring = req.collider_ring;
        let detail_upsample = req.detail_upsample.max(1);
        // The terrain's composed layer stack (from its USD child layer prims). Stamp
        // layers (craters, …) run OFF-THREAD in the build task below; scatter layers
        // (rocks) are no-ops here and run on the main thread post-build.
        let layers = stack.map(|s| s.0.clone()).unwrap_or_default();

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
            let mut tile = if target_res > 0 && target_res < native_res {
                resample(&tile, tile.half_extent as f64, target_res)
            } else {
                tile
            };
            // INTELLIGENT UPSCALING: the DEM ground is coarse (~5 m) but generated
            // craters are procedural and deserve higher fidelity. Bilinearly upscale
            // the coarse ground to a finer working grid (no fake ground detail — just
            // smoother interpolation), THEN stamp craters into it so their rims resolve
            // well below the DEM sampling. Decouples crater fidelity from DEM res.
            if detail_upsample > 1 {
                let target = (tile.res - 1) * detail_upsample + 1;
                tile = resample(&tile, tile.half_extent as f64, target);
            }
            let half_extent = tile.half_extent;
            let res = tile.res;
            // Retain the upscaled-but-crater-FREE grid so a live Inspector regenerate
            // re-stamps craters off it at full detail without re-reading the GeoTIFF.
            let base_grid = std::sync::Arc::new(tile.clone());
            // Apply the geometry STAMP layers (craters, …) into the working grid BEFORE
            // the collider/mesh derive, so both the streamed tiles and the heightfield
            // collider show (and collide with) the same features. Deterministic from
            // the spec seed; the source DEM bytes are never touched. Each layer logs
            // its own contribution.
            for layer in &layers {
                layer.stamp(&mut tile);
            }
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
                base_grid,
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
        // Retain the pristine base grid + source settings so the crater layer can be
        // re-baked live from the Inspector (`RegenerateField`) without disk I/O.
        e.insert((
            DemBaseGrid(built.base_grid),
            DemTerrainSource { collider_ring: req.collider_ring },
        ));
        // Retain the grid + mark the streaming mode(s). `lod_viz` streams visual LOD
        // tiles (static mesh suppressed); `collider_ring` streams physics tiles
        // (static collider suppressed above). Both sample the retained `DemHeightField`.
        if let Some(grid) = built.grid {
            e.insert(crate::stream_viz::DemHeightField(grid));
            if req.lod_viz {
                e.insert((
                    crate::stream_viz::TerrainLodViz::default(),
                    crate::stream_viz::LodTiles::default(),
                    crate::stream_viz::PendingTileBakes::default(),
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

/// Live **re-bake** of the DEM crater + rock layers when the shared
/// [`ObstacleFieldSpec`] changes (Inspector edit / networked `UpdateObstacleFieldSpec`
/// → [`RegenerateField`]). This is what makes the *one* obstacle-field Inspector
/// drive the DEM: no GeoTIFF re-read — craters re-stamp off the retained
/// [`DemBaseGrid`], the new grid is swapped into [`DemHeightField`], the static
/// collider is rebuilt, and the streamed visual tiles + their mesh cache are
/// invalidated so they re-bake from the new heights. Rocks re-scatter by clearing
/// [`RocksScattered`] (the scatter system re-runs next frame).
#[allow(clippy::too_many_arguments)]
fn regenerate_dem_layers(
    mut events: MessageReader<RegenerateTerrainLayers>,
    mut commands: Commands,
    mut terrains: Query<(
        Entity,
        &DemBaseGrid,
        &DemTerrainSource,
        Ref<crate::terrain_layers::TerrainLayerStack>,
        &mut crate::stream_viz::DemHeightField,
        Option<&mut crate::stream_viz::LodTiles>,
        Has<Mesh3d>,
    )>,
    scattered: Query<Entity, With<crate::terrain_layers::TerrainScatterEntity>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    // Re-bake a terrain when its layer stack CHANGED (a USD layer prim was edited →
    // the stack was re-parsed + re-inserted; change detection avoids command-ordering
    // races a one-shot message would have), or when a force message is fired.
    let forced = !events.is_empty();
    events.clear();

    let mut any = false;
    for (entity, base, src, stack, mut hf, tiles, has_static_mesh) in &mut terrains {
        if !forced && !stack.is_changed() {
            continue;
        }
        any = true;
        // Re-stamp the stack's geometry layers off the pristine (upscaled, crater-free)
        // base grid — no disk I/O, full crater detail preserved.
        let mut grid = (*base.0).clone();
        for layer in &stack.0 {
            layer.stamp(&mut grid);
        }
        let half = grid.half_extent as f64;

        // Rebuild the static heightfield collider (skip when a collider ring streams
        // physics — it re-derives from the swapped DemHeightField below).
        if !src.collider_ring {
            let collider =
                Collider::heightfield(grid.to_avian_heights(), DVec3::new(2.0 * half, 1.0, 2.0 * half));
            commands.entity(entity).insert((RigidBody::Static, collider));
        }

        // Rebuild the static visual mesh, if this terrain uses one (not streaming).
        if has_static_mesh {
            if let Some(meshes) = meshes.as_mut() {
                let MeshData { positions, normals, uvs, indices } = grid.to_mesh_data();
                commands.entity(entity).insert(Mesh3d(meshes.add(
                    lunco_obstacle_field::grid_mesh(positions, normals, uvs, indices),
                )));
            }
        }

        // Swap in the new height grid (streaming tiles, collider ring, and the
        // TerrainHeight query all sample this).
        *hf = crate::stream_viz::DemHeightField(std::sync::Arc::new(grid));

        // Invalidate streamed visual tiles: despawn the live set + drop in-flight
        // bakes so update_lod_tiles re-bakes from the new heights.
        if let Some(mut tiles) = tiles {
            for (_, e) in tiles.0.drain() {
                commands.entity(e).try_despawn();
            }
            commands.entity(entity).insert(crate::stream_viz::PendingTileBakes::default());
        }

        // Scatter layers re-run once the applied-marker is gone (next frame).
        commands.entity(entity).remove::<crate::terrain_layers::TerrainLayersApplied>();

        info!("[dem-terrain] regenerated terrain layers (±{:.0} m)", half);
    }

    if any {
        // The cached tile meshes are stale now → drop them so re-baked tiles pick up
        // the new heights (the cache is keyed by quadtree node, not terrain).
        commands.insert_resource(crate::stream_viz::LodMeshCache::default());
        // Despawn old scatter entities (rocks, …); scatter rebuilds from the new spec.
        for e in &scattered {
            commands.entity(e).try_despawn();
        }
    }
}

/// Fire to force a re-bake of every layered DEM terrain from its current
/// [`TerrainLayerStack`]. Re-stamps off the retained base grid + re-scatters (no
/// GeoTIFF re-read). The usual live-edit path needs no message: editing a layer prim
/// re-parses + re-inserts the stack, and `regenerate_dem_layers` picks that up via
/// `Changed<TerrainLayerStack>`. This message is the explicit force path for when the
/// stack content is unchanged but a re-bake is still wanted.
#[derive(Message, Default)]
pub struct RegenerateTerrainLayers;

/// Live terrain tuning from the Inspector's "Craters & Rocks" panel: when the
/// shared [`ObstacleFieldSpec`] is edited (the panel fires
/// [`UpdateObstacleFieldSpec`]), rebuild every DEM terrain's crater/rock layers
/// from the new spec. Mutating the stack trips `Changed<TerrainLayerStack>`, so
/// [`regenerate_dem_layers`] re-bakes incrementally off the retained base grid —
/// no GeoTIFF re-read, no scene reload (so the live world is never duplicated).
fn on_obstacle_spec_rebuild_layers(
    trigger: On<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack>,
) {
    let spec = trigger.event().spec.clone();
    for mut stack in &mut terrains {
        crate::terrain_layers::apply_obstacle_spec_to_stack(&mut stack, &spec);
    }
}

/// Register the DEM-terrain command + spawn systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.register_type::<SpawnDemTerrain>()
        .add_message::<RegenerateTerrainLayers>()
        .add_observer(on_obstacle_spec_rebuild_layers)
        .add_systems(Update, (start_dem_builds, finish_dem_builds, regenerate_dem_layers));
    register_all_commands(app);
}
