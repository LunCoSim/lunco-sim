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

/// Marker: this terrain's authoritative state lives in a **document** — edits are
/// authored there (journaled, undoable, synced) and *project* into the runtime layer,
/// so live-edit commands must NOT mutate the `TerrainLayerStack` directly for it. A
/// terrain WITHOUT this marker is document-free (quick-spawned, headless, tests) and
/// edits apply directly to the runtime layer. The assembly crate that owns the USD
/// document attaches this once it resolves the backing doc; terrain-surface stays
/// USD-agnostic (it only reads the marker, never the document).
#[derive(Component)]
pub struct DocBackedTerrain;

/// The deterministic crater placements for a terrain of the given `half_extent` —
/// the set the craters layer turns into its analytic [`lunco_terrain_core::Craters`]
/// modifier on the surface oracle. **Non-destructive** (the source DEM is never
/// touched) and **deterministic** (the seed drives placement, so every networked
/// peer regenerates identical basins with nothing to transfer).
///
/// Craters fill the WHOLE DEM window (`half_extent`), not the spec's
/// `region_half_extent` (which bounds the near-field rock scatter). Placement is
/// **complete spatial randomness** (unconstrained uniform) — NOT the spec's
/// `pattern` (which tunes rocks) and NOT blue noise: real crater populations are
/// Poisson-distributed, with overlapping pairs, chains, and bare stretches. An
/// enforced min-spacing near the mean spacing packs the field into a near-lattice
/// that reads as a repeating carpet. Overlaps are wanted — additive deltas on a
/// rare coincident pair read as a merged doublet or an extra-deep fresh crater
/// (saturation equilibrium IS overlapping craters), and the collider firewall
/// slope-limits anything extreme. Pure → safe to call off-thread.
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
    sample_layer(
        seed,
        salt::CRATERS,
        Pattern::Uniform,
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
        stack.push_layer("craters", crate::terrain_layers::make_crater_layer(ev.crater_density, 22.0, 0.3, 0xC0FFEE));
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

/// Dig or raise the terrain with a smooth radial brush: `amplitude` metres at the
/// centre — **negative digs, positive raises** — falling to zero at `radius`. `(x, z)`
/// are terrain-local metres. Appends an edit layer to every DEM terrain; the
/// `Changed<TerrainLayerStack>` re-bake lands it in the tiles AND the collider, so the
/// rover drives exactly the hole/berm you see. Reachable from rhai / API / MCP as a
/// command: `cmd("BrushTerrain", #{x, z, radius, amplitude})`.
/// Monotonic per-session counter minting an **explicit** stable id for each runtime
/// edit (`edit/brush#7`) — never reused, so removing one edit can't collide with a
/// later one. This is the runtime stand-in for a USD prim path: once edits are
/// authored as prims, the id becomes the path and this counter goes away.
#[derive(Resource, Default)]
pub struct TerrainEditSeq(pub u64);

impl TerrainEditSeq {
    fn next(&mut self, kind: &str) -> crate::terrain_layers::LayerId {
        let id = crate::terrain_layers::LayerId::new(format!("edit/{kind}#{}", self.0));
        self.0 += 1;
        id
    }
}

/// Resolve the layer id for an edit: caller-supplied when non-empty, else the next
/// monotonic handle. Explicit either way — no synthesised heuristic.
fn edit_id(explicit: &str, kind: &str, seq: &mut TerrainEditSeq) -> crate::terrain_layers::LayerId {
    if explicit.is_empty() {
        seq.next(kind)
    } else {
        crate::terrain_layers::LayerId::new(explicit.to_string())
    }
}

#[Command(default)]
pub struct BrushTerrain {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    pub amplitude: f32,
    /// Optional stable id for the edit (so it can be removed later). Empty = auto.
    pub id: String,
}

#[on_command(BrushTerrain)]
fn on_brush_terrain(
    trigger: On<BrushTerrain>,
    // Document-free terrains only — a doc-backed terrain's edits are authored to its
    // USD document (the authoring tier) and project in, so we must not mutate it here.
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
    mut seq: ResMut<TerrainEditSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let id = edit_id(&ev.id, "brush", &mut seq);
    let kind = crate::terrain_layers::EditKind::Brush {
        center: [ev.x as f64, ev.z as f64],
        radius: ev.radius as f64,
        amplitude: ev.amplitude as f64,
    };
    // TODO(terrain-as-USD-doc): author this as a USD doc op on the terrain's edits
    // prim (record→journal→project), not a direct ECS mutation. Applies to every
    // terrain for now; scope by the terrain's document/prim path.
    for mut stack in &mut terrains {
        stack.add_edit(id.clone(), kind);
    }
}

/// Flatten the terrain toward `target_y` within `radius`, blending back to the
/// existing surface at the edge — the "level a landing pad" tool. `(x, z)` are
/// terrain-local metres. Reachable as `cmd("FlattenTerrain", #{x, z, radius, target_y})`.
#[Command(default)]
pub struct FlattenTerrain {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    pub target_y: f32,
    /// Optional stable id for the edit (so it can be removed later). Empty = auto.
    pub id: String,
}

#[on_command(FlattenTerrain)]
fn on_flatten_terrain(
    trigger: On<FlattenTerrain>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
    mut seq: ResMut<TerrainEditSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let id = edit_id(&ev.id, "flatten", &mut seq);
    let kind = crate::terrain_layers::EditKind::Flatten {
        center: [ev.x as f64, ev.z as f64],
        radius: ev.radius as f64,
        target_y: ev.target_y as f64,
    };
    for mut stack in &mut terrains {
        stack.add_edit(id.clone(), kind);
    }
}

/// Remove a terrain layer by its [`LayerId`] — undo a specific dig/flatten (or any
/// addressable layer). Re-bakes via `Changed<TerrainLayerStack>`. Reachable as
/// `cmd("RemoveTerrainLayer", #{id})`.
#[Command(default)]
pub struct RemoveTerrainLayer {
    pub id: String,
}

#[on_command(RemoveTerrainLayer)]
fn on_remove_terrain_layer(
    trigger: On<RemoveTerrainLayer>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
) {
    let id = crate::terrain_layers::LayerId::new(trigger.event().id.clone());
    for mut stack in &mut terrains {
        // An edit inside the consolidated layer, or a whole authored layer.
        if !stack.remove_edit(&id) {
            stack.remove_layer(&id);
        }
    }
}

register_commands!(
    on_spawn_dem_terrain,
    on_brush_terrain,
    on_flatten_terrain,
    on_remove_terrain_layer
);

/// What the off-thread build produces, ready to assemble into entities.
struct DemBuild {
    /// The static visual mesh — `None` in `lod_viz` mode (tiles draw instead).
    mesh: Option<MeshData>,
    /// The static heightfield collider, **built off-thread** — `None` in
    /// `collider_ring` mode (a streamed collider ring replaces it). Constructing the
    /// parry heightfield for a multi-million-point DEM is expensive, so it happens in
    /// the build task; the main thread only inserts the finished component.
    collider: Option<Collider>,
    /// The composed surface oracle (raster base + analytic crater/edit modifiers),
    /// always retained as the ONE `HeightSource` the streaming consumers, the
    /// derived-layer bakes, and the `TerrainHeight` query sample.
    oracle: Option<std::sync::Arc<crate::oracle::SurfaceOracle>>,
    /// The raster base grid BEFORE any layer folded in — retained so a live
    /// layer edit re-composes off it without re-reading the GeoTIFF.
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
        // The terrain's composed layer stack (from its USD child layer prims).
        // Height layers (craters, edits) contribute ANALYTIC modifiers to the
        // surface oracle OFF-THREAD in the build task below; scatter layers
        // (rocks) are no-ops here and run on the main thread post-build.
        let layers: Vec<_> =
            stack.map(|s| s.0.iter().map(|e| e.layer.clone()).collect()).unwrap_or_default();

        let task = AsyncComputeTaskPool::get().spawn(async move {
            let meta_bytes = read_bytes(meta_path).await?;
            let meta_str = String::from_utf8(meta_bytes)
                .map_err(|e| format!("metadata.yaml not utf-8: {e}"))?;
            let meta = DemMetadata::from_yaml_str(&meta_str).map_err(|e| e.to_string())?;

            let tif = read_bytes(tif_path).await?;
            let grid = height_grid_from_geotiff(&tif, &meta).map_err(|e| e.to_string())?;

            // Crop the playable region at native resolution. The mesh and collider
            // share this surface so visuals and contact agree.
            let tile = crop_centered(&grid, half_window);
            let native_res = tile.res;
            // Optional visual-quality downsample (lossy). 0 / ≥ native keeps native.
            let mut tile = if target_res > 0 && target_res < native_res {
                resample(&tile, tile.half_extent as f64, target_res)
            } else {
                tile
            };
            let half_extent = tile.half_extent;
            let res = tile.res;
            // Retain the pristine raster base so a live layer edit re-composes off
            // it without re-reading the GeoTIFF.
            let base_grid = std::sync::Arc::new(tile.clone());
            // Fold any genuinely-rasterising layers into the working grid (most
            // height layers contribute analytically below instead).
            for layer in &layers {
                layer.stamp(&mut tile);
            }
            // Compose the surface oracle: raster base + each layer's ANALYTIC
            // height modifier, in stack (USD prim) order. The tile baker, the
            // collider ring, the derived-layer bakes, and the TerrainHeight query
            // all sample this ONE source — crater rims resolve at each consumer's
            // own sampling density, unbounded by the DEM grid. Deterministic from
            // the spec seeds; the source DEM bytes are never touched.
            let contributions: Vec<_> =
                layers.iter().filter_map(|l| l.height_modifier(half_extent)).collect();
            let oracle = std::sync::Arc::new(crate::oracle::SurfaceOracle::new(
                std::sync::Arc::new(tile),
                contributions,
            ));
            // Static consumers rasterise the composed surface once at base
            // resolution; streaming consumers sample the oracle directly.
            let needs_static_grid = !collider_ring || !lod_viz;
            let materialized = needs_static_grid.then(|| oracle.materialize());
            // Build the static heightfield collider HERE (off-thread) when this isn't
            // a streamed collider ring — the parry heightfield build over a
            // multi-million-point DEM is the load-time cost that used to stall the
            // main thread in `finish_dem_builds`.
            let collider = if collider_ring {
                None
            } else {
                let g = materialized.as_ref().expect("materialized for static collider");
                let h = g.half_extent as f64;
                Some(Collider::heightfield(g.to_avian_heights(), DVec3::new(2.0 * h, 1.0, 2.0 * h)))
            };
            // Static visual mesh unless lod_viz streams visual tiles instead.
            let mesh = if lod_viz {
                None
            } else {
                materialized.as_ref().map(|g| g.to_mesh_data())
            };
            Ok(DemBuild {
                collider,
                mesh,
                oracle: Some(oracle),
                base_grid,
                half_extent,
                res,
                native_res,
                site: meta.site_id,
            })
        });
        commands.entity(entity).try_insert(DemBuildTask(task));
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
        commands.entity(entity).try_remove::<(DemBuildTask, DemTerrainRequest)>();

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
        // Static full-DEM collider — already built off-thread (`None` when a collider
        // ring streams per-tile colliders instead). Just insert the finished component.
        if let Some(collider) = built.collider {
            e.try_insert((RigidBody::Static, collider));
        }
        // Retain the pristine base grid + source settings so the crater layer can be
        // re-baked live from the Inspector (`RegenerateField`) without disk I/O.
        e.try_insert((
            DemBaseGrid(built.base_grid),
            DemTerrainSource { collider_ring: req.collider_ring },
        ));
        // Retain the oracle + mark the streaming mode(s). `lod_viz` streams visual LOD
        // tiles (static mesh suppressed); `collider_ring` streams physics tiles
        // (static collider suppressed above). Both sample the retained `DemHeightField`.
        if let Some(oracle) = built.oracle {
            e.try_insert(crate::stream_viz::DemHeightField(oracle));
            if req.lod_viz {
                e.try_insert((
                    crate::stream_viz::TerrainLodViz::default(),
                    crate::stream_viz::LodTiles::default(),
                    crate::stream_viz::PendingTileBakes::default(),
                    crate::stream_viz::TerrainNodeErrors::default(),
                    // Default Lit; switchable live in the Inspector (Terrain Shader).
                    crate::stream_viz::TerrainShaderMode::default(),
                ));
            }
            if req.collider_ring {
                e.try_insert((
                    crate::collider_ring::TerrainColliderRing::default(),
                    crate::collider_ring::ColliderTiles::default(),
                    crate::collider_ring::PendingColliderBakes::default(),
                ));
            }
        }
        if let (Some(meshes), Some(mesh)) = (meshes.as_mut(), built.mesh) {
            let MeshData { positions, normals, uvs, indices } = mesh;
            e.try_insert(Mesh3d(meshes.add(lunco_obstacle_field::grid_mesh(
                positions, normals, uvs, indices,
            ))));
            // Default material only for the standalone command path; the USD path
            // authors its own via `materialType` (don't clobber it).
            if req.with_default_material {
                if let Some(materials) = materials.as_mut() {
                    e.try_insert(MeshMaterial3d(materials.add(StandardMaterial {
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

/// In-flight off-thread **visual** re-stamp for a terrain: clones the pristine base
/// grid and stamps the layers into it, producing the new working [`HeightGrid`]. This
/// is the FAST path (clone + stamp only); the expensive static-collider rebuild is
/// split off into a separate debounced task ([`DemColliderDirty`]/[`DemColliderTask`])
/// so the visible terrain updates ~immediately and physics catches up after.
#[derive(Component)]
struct DemRestampTask(Task<std::sync::Arc<crate::oracle::SurfaceOracle>>);

/// Armed after a visual re-stamp swaps new heights in (non-collider-ring terrains):
/// once it settles, the static heightfield collider is rebuilt off-thread from the
/// current grid. Decoupling it from the visual swap means dragging a slider doesn't
/// wait on (or repeatedly redo) the multi-million-point collider build — physics just
/// reconverges shortly after the visuals.
#[derive(Component)]
struct DemColliderDirty(Timer);

/// In-flight off-thread static-collider rebuild (see [`DemColliderDirty`]).
#[derive(Component)]
struct DemColliderTask(Task<Collider>);

/// Settle delay before the (heavy) static collider is rebuilt after the last visual
/// re-stamp. Longer than the restamp debounce so a burst of edits rebuilds the
/// collider just once, after the heights stop changing.
const COLLIDER_DEBOUNCE_SECS: f32 = 0.6;

/// Set when a re-bake is requested while one is already running → coalesce: when the
/// current task finishes, run exactly one more with the latest stack. So dragging a
/// slider streams responsively (first edit starts immediately) and settles on a final
/// re-bake, instead of queueing a full re-stamp every frame of the drag.
#[derive(Component)]
struct DemRestampPending;

/// Debounce armed on each layer-stack change; the re-stamp only kicks off once it
/// elapses with no further change. Re-stamping the *whole* DEM is heavy (clone +
/// tens of thousands of crater stamps + collider build), so coalescing a slider
/// drag's many changes into ONE trailing re-stamp is what keeps live tuning from
/// piling up back-to-back full re-bakes (the "it stuck" when changing repeatedly).
#[derive(Component)]
struct DemRestampDebounce(Timer);

/// Settle delay before a layer edit triggers the off-thread re-stamp. Long enough to
/// swallow a continuous slider drag, short enough to feel responsive on release.
const RESTAMP_DEBOUNCE_SECS: f32 = 0.3;

/// Spawn the off-thread **re-compose** task: rebuild the surface oracle from the
/// pristine [`DemBaseGrid`] + the stack's current layers. With analytic height
/// layers this is CHEAP (placement generation, no grid-wide rasterisation) — a
/// raster `stamp` layer still folds into a cloned base first. Just the oracle —
/// the static-collider rebuild is deferred ([`DemColliderDirty`]) so the visuals
/// don't wait on it.
fn spawn_restamp_task(
    commands: &mut Commands,
    entity: Entity,
    base: &DemBaseGrid,
    stack: &crate::terrain_layers::TerrainLayerStack,
) {
    let base_grid = base.0.clone();
    let layers: Vec<_> = stack.0.iter().map(|e| e.layer.clone()).collect();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        let half_extent = base_grid.half_extent;
        let mut grid = (*base_grid).clone();
        for layer in &layers {
            layer.stamp(&mut grid);
        }
        let contributions: Vec<_> =
            layers.iter().filter_map(|l| l.height_modifier(half_extent)).collect();
        std::sync::Arc::new(crate::oracle::SurfaceOracle::new(
            std::sync::Arc::new(grid),
            contributions,
        ))
    });
    commands.entity(entity).try_insert(DemRestampTask(task));
}

/// Live **re-bake** of the DEM crater + rock layers when the shared
/// [`ObstacleFieldSpec`] changes (Inspector edit / networked `UpdateObstacleFieldSpec`)
/// or a [`RegenerateTerrainLayers`] force message fires. This is what makes the *one*
/// obstacle-field Inspector drive the DEM: no GeoTIFF re-read — craters re-stamp off
/// the retained [`DemBaseGrid`]. The expensive work runs OFF-THREAD ([`spawn_restamp_task`])
/// so the edit never freezes the frame; [`finish_dem_restamp`] swaps the result in.
#[allow(clippy::type_complexity)]
fn start_dem_restamp(
    mut events: MessageReader<RegenerateTerrainLayers>,
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &DemBaseGrid,
        Ref<crate::terrain_layers::TerrainLayerStack>,
        Has<DemRestampTask>,
        Option<&mut DemRestampDebounce>,
    )>,
) {
    // Re-bake when the stack CHANGED (a layer prim / the Inspector spec was edited →
    // the stack was re-parsed + re-inserted; change detection avoids the
    // command-ordering races a one-shot message would have), or when forced.
    let forced = !events.is_empty();
    events.clear();
    for (entity, base, stack, busy, debounce) in &mut q {
        if forced || stack.is_changed() {
            // (Re)arm the debounce on every change so a continuous drag keeps
            // pushing the deadline out → exactly one re-stamp once the drag stops.
            match debounce {
                Some(mut d) => {
                    d.0.set_duration(std::time::Duration::from_secs_f32(RESTAMP_DEBOUNCE_SECS));
                    d.0.reset();
                }
                None => {
                    commands.entity(entity).try_insert(DemRestampDebounce(Timer::from_seconds(
                        RESTAMP_DEBOUNCE_SECS,
                        TimerMode::Once,
                    )));
                }
            }
            continue;
        }
        // No change this frame: tick the pending debounce and fire when it settles.
        let Some(mut d) = debounce else { continue };
        if !d.0.tick(time.delta()).just_finished() {
            continue;
        }
        commands.entity(entity).try_remove::<DemRestampDebounce>();
        if busy {
            // A re-stamp is still running → mark for one coalesced trailing run.
            commands.entity(entity).try_insert(DemRestampPending);
            continue;
        }
        spawn_restamp_task(&mut commands, entity, base, &stack);
    }
}

/// Collect finished off-thread re-stamps: swap in the new heights + collider, then
/// trigger a **progressive** visual refresh — bump the streaming generation so live
/// tiles go stale and re-bake near-camera-first (covering the surface meanwhile)
/// rather than all being despawned at once. Rocks/overlays re-scatter next frame.
#[allow(clippy::type_complexity)]
fn finish_dem_restamp(
    mut commands: Commands,
    mut tasks: Query<(
        Entity,
        &mut DemRestampTask,
        &DemTerrainSource,
        &mut crate::stream_viz::DemHeightField,
        Option<&mut crate::stream_viz::LodTiles>,
        Option<&mut crate::stream_viz::PendingTileBakes>,
        Has<Mesh3d>,
        Has<DemRestampPending>,
        Option<&TerrainDirty>,
    )>,
    scattered: Query<Entity, With<crate::terrain_layers::TerrainScatterEntity>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut mesh_cache: ResMut<crate::stream_viz::LodMeshCache>,
) {
    use bevy::tasks::futures_lite::future;
    // Whether ANY terrain did a WHOLE-terrain re-bake this pass (spec change / load) —
    // only then do the (global) scatter entities need dropping + rebuilding.
    let mut any_full = false;
    for (entity, mut task, src, mut hf, tiles, pending, has_static_mesh, was_pending, dirty) in
        &mut tasks
    {
        let Some(oracle) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).try_remove::<DemRestampTask>();

        // The region this re-bake must refresh: `Some` = a bounded edit (only those
        // tiles + no rock re-scatter); `None`/absent = whole terrain.
        let dirty_bounds = dirty.and_then(|d| d.bounds);
        let scoped = dirty_bounds.is_some();
        // Consume the mark so the NEXT change starts clean.
        commands.entity(entity).try_remove::<TerrainDirty>();

        let half = oracle.half_extent() as f64;

        // Defer the (heavy) static-collider rebuild: arm its debounce instead of
        // building it here, so the VISUAL swap below lands immediately and physics
        // reconverges shortly after. Collider-ring terrains stream physics → no
        // static collider to rebuild.
        if !src.collider_ring {
            commands.entity(entity).try_insert(DemColliderDirty(Timer::from_seconds(
                COLLIDER_DEBOUNCE_SECS,
                TimerMode::Once,
            )));
        }
        // Rebuild the static visual mesh, if this terrain uses one (not streaming).
        if has_static_mesh {
            if let Some(meshes) = meshes.as_mut() {
                let MeshData { positions, normals, uvs, indices } = oracle.materialize().to_mesh_data();
                commands.entity(entity).try_insert(Mesh3d(meshes.add(
                    lunco_obstacle_field::grid_mesh(positions, normals, uvs, indices),
                )));
            }
        }
        // Hand the edited region to the collider ring so it re-bakes ONLY the tiles the
        // edit touched (and skips rings the edit doesn't reach), instead of despawning +
        // rebuilding every ring tile on any oracle swap (the burst physics spike). Keyed
        // by the new oracle's `surface_key` so `update_collider_ring` matches this exact
        // swap; captured BEFORE the oracle moves into `hf`. `None` bounds = whole terrain.
        let new_oracle_key = oracle.surface_key();
        commands.entity(entity).try_insert(crate::collider_ring::ColliderDirtyRegion {
            bounds: dirty_bounds,
            oracle_key: new_oracle_key,
        });
        // Swap in the new surface (streaming tiles, collider ring, TerrainHeight query).
        *hf = crate::stream_viz::DemHeightField(oracle);

        // Progressive refresh: bump the generation so live tiles go stale + re-bake
        // near-first (still covering the surface), and drop in-flight bakes from the
        // OLD heights. No despawn-everything flash. First REAP any tiles already stale
        // from a prior re-bake so rapid edits keep at most one generation of cover —
        // otherwise dead tiles pile up and the per-frame bookkeeping goes O(n²).
        if let Some(mut tiles) = tiles {
            for e in tiles.reap_stale() {
                commands.entity(e).try_despawn();
            }
            // Only tiles overlapping the edited patch go stale + re-bake; a whole-
            // terrain change (`None`) invalidates all, as before.
            tiles.invalidate_region(dirty_bounds, half);
        }
        if let Some(mut pending) = pending {
            *pending = crate::stream_viz::PendingTileBakes::default();
        }
        // The per-node mesh cache assumes geometry is a pure function of the coord,
        // which a live edit breaks — drop the entries the edit touched (or all, on a
        // whole-terrain change) so re-baked tiles pick up the new heights.
        mesh_cache.drop_region(dirty_bounds, half);

        // A bounded edit leaves the crater/rock fields untouched, so DON'T re-scatter
        // (that despawn+respawn of every rock is a big part of the per-edit cost). A
        // whole-terrain change re-scatters: clear the applied-marker so scatter re-runs.
        if !scoped {
            commands.entity(entity).try_remove::<crate::terrain_layers::TerrainLayersApplied>();
            any_full = true;
        }

        // Coalesced trailing re-bake (a change arrived mid-task): re-arm the debounce
        // rather than spawning immediately, so a still-active drag keeps coalescing.
        if was_pending {
            commands.entity(entity).try_remove::<DemRestampPending>();
            commands.entity(entity).try_insert(DemRestampDebounce(Timer::from_seconds(
                RESTAMP_DEBOUNCE_SECS,
                TimerMode::Once,
            )));
        }
        info!("[dem-terrain] regenerated terrain layers (±{:.0} m)", half);
    }

    if any_full {
        // A whole-terrain re-bake happened (spec change / load): every scatter entity
        // (rocks, crater overlays) may have moved, so drop them and let the scatter
        // layers rebuild. Bounded edits skip this entirely (rocks are unchanged).
        for e in &scattered {
            commands.entity(e).try_despawn();
        }
    }
}

/// Debounced **static-collider rebuild**, decoupled from the visual re-stamp. When a
/// re-stamp swaps new heights in it arms [`DemColliderDirty`]; once that settles (no
/// further re-stamp), this rebuilds the heightfield collider OFF-THREAD from the
/// current grid. So a slider drag updates the visible terrain right away and physics
/// reconverges a moment later — instead of every edit blocking on the multi-million-
/// point collider build. Skips while a build is already in flight (it'll re-arm).
fn start_dem_collider(
    time: Res<Time>,
    mut commands: Commands,
    mut q: Query<(
        Entity,
        &crate::stream_viz::DemHeightField,
        Has<DemColliderTask>,
        &mut DemColliderDirty,
    )>,
) {
    for (entity, hf, busy, mut dirty) in &mut q {
        if !dirty.0.tick(time.delta()).just_finished() {
            continue;
        }
        commands.entity(entity).try_remove::<DemColliderDirty>();
        if busy {
            // A build is still running with older heights → re-arm so we rebuild once
            // more from the latest grid when it frees up.
            commands.entity(entity).try_insert(DemColliderDirty(Timer::from_seconds(
                COLLIDER_DEBOUNCE_SECS,
                TimerMode::Once,
            )));
            continue;
        }
        let oracle = hf.0.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // Rasterise the composed surface at base resolution for the one static
            // heightfield (streamed rings sample the oracle per-tile instead).
            let grid = oracle.materialize();
            let h = grid.half_extent as f64;
            Collider::heightfield(grid.to_avian_heights(), DVec3::new(2.0 * h, 1.0, 2.0 * h))
        });
        commands.entity(entity).try_insert(DemColliderTask(task));
    }
}

/// Insert finished off-thread static colliders (see [`start_dem_collider`]).
fn finish_dem_collider(
    mut commands: Commands,
    mut q: Query<(Entity, &mut DemColliderTask)>,
) {
    use bevy::tasks::futures_lite::future;
    for (entity, mut task) in &mut q {
        let Some(collider) = future::block_on(future::poll_once(&mut task.0)) else {
            continue;
        };
        commands
            .entity(entity)
            .try_remove::<DemColliderTask>()
            .insert((RigidBody::Static, collider));
    }
}

/// Fire to force a re-bake of every layered DEM terrain from its current
/// [`TerrainLayerStack`]. Re-stamps off the retained base grid + re-scatters (no
/// GeoTIFF re-read). The usual live-edit path needs no message: editing a layer prim
/// re-parses + re-inserts the stack, and `start_dem_restamp` picks that up via
/// `Changed<TerrainLayerStack>`. This message is the explicit force path for when the
/// stack content is unchanged but a re-bake is still wanted.
#[derive(Message, Default)]
pub struct RegenerateTerrainLayers;

/// Live terrain tuning from the Inspector's "Craters & Rocks" panel: when the
/// shared [`ObstacleFieldSpec`] is edited (the panel fires
/// [`UpdateObstacleFieldSpec`]), rebuild every **document-free** DEM terrain's
/// crater/rock layers from the new spec. Mutating the stack trips
/// `Changed<TerrainLayerStack>`, so [`start_dem_restamp`] re-bakes OFF-THREAD off the
/// retained base grid and refreshes the tiles progressively — no GeoTIFF re-read.
///
/// **Doc-backed terrains are excluded** (`Without<DocBackedTerrain>`): their crater/
/// rock layers are USD-authored (`lunco:layer` prims) and owned by the projection, so
/// mutating the stack directly would (a) fight the next USD re-projection and (b) run
/// a full re-stamp of a possibly huge terrain (e.g. the ±4 km moonbase — 10 M verts)
/// on every slider tick. Tuning a doc-backed terrain's craters must author to its USD
/// crater prim instead (SetAttribute → project → bounded re-stamp), the same
/// authoring tier the edit commands use — TODO, and it needs bounded re-bake to be
/// cheap on large terrains.
fn on_obstacle_spec_rebuild_layers(
    trigger: On<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
) {
    let spec = trigger.event().spec.clone();
    for mut stack in &mut terrains {
        crate::terrain_layers::apply_obstacle_spec_to_stack(&mut stack, &spec);
    }
}

// ── Incremental region re-bake ────────────────────────────────────────────────

/// The region a pending re-bake must refresh, accumulated from the edits since the
/// last bake. `Some([min_x, min_z, max_x, max_z])` (terrain-local metres) = a bounded
/// patch (a brush/flatten touches only `center ± radius`): [`finish_dem_restamp`]
/// re-bakes only the tiles overlapping it and leaves the rock scatter alone. `None`
/// — or the component absent — = whole terrain (a spec change / first load).
/// Consumed (removed) by the re-bake. This is what turns an edit into an O(patch)
/// re-bake instead of O(whole map). New layer types get incremental re-bake for free
/// by declaring their footprint here; undo/redo scopes via the removed edit's bounds.
#[derive(Component, Default)]
pub struct TerrainDirty {
    /// Union of dirtied AABBs, or `None` = whole terrain (sticky once whole).
    pub bounds: Option<[f64; 4]>,
}

impl TerrainDirty {
    /// Grow the dirty region by a bounded edit's AABB. A whole-terrain (`None`) mark
    /// is a superset, so a later bounded patch can never shrink it back.
    fn grow(&mut self, aabb: [f64; 4]) {
        if let Some(b) = &mut self.bounds {
            b[0] = b[0].min(aabb[0]);
            b[1] = b[1].min(aabb[1]);
            b[2] = b[2].max(aabb[2]);
            b[3] = b[3].max(aabb[3]);
        }
    }
}

/// Accumulate `mark` onto every re-stampable DEM terrain (doc-free AND doc-backed —
/// this keys off the edit *command*, not which apply-path ran). `Some(aabb)` grows the
/// patch; `None` marks the whole terrain.
fn accumulate_terrain_dirty(
    commands: &mut Commands,
    q: &mut Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
    mark: Option<[f64; 4]>,
) {
    for (e, dirty) in q.iter_mut() {
        match (dirty, mark) {
            (Some(mut d), Some(aabb)) => d.grow(aabb),
            (Some(mut d), None) => d.bounds = None, // whole terrain wins (sticky)
            (None, m) => {
                commands.entity(e).try_insert(TerrainDirty { bounds: m });
            }
        }
    }
}

fn edit_command_aabb(x: f32, z: f32, radius: f32) -> [f64; 4] {
    [(x - radius) as f64, (z - radius) as f64, (x + radius) as f64, (z + radius) as f64]
}

/// Mark a brush edit's footprint dirty (see [`TerrainDirty`]). Registered alongside
/// [`on_brush_terrain`] / the doc-backed authoring observer — it only records the
/// region; the apply-path does the height change.
fn on_brush_terrain_dirty(
    trigger: On<BrushTerrain>,
    mut commands: Commands,
    mut q: Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    accumulate_terrain_dirty(&mut commands, &mut q, Some(edit_command_aabb(ev.x, ev.z, ev.radius)));
}

/// Mark a flatten edit's footprint dirty (see [`TerrainDirty`]).
fn on_flatten_terrain_dirty(
    trigger: On<FlattenTerrain>,
    mut commands: Commands,
    mut q: Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    accumulate_terrain_dirty(&mut commands, &mut q, Some(edit_command_aabb(ev.x, ev.z, ev.radius)));
}

/// Scope an undo/remove to the removed edit's footprint (see [`TerrainDirty`]). A
/// removed brush/flatten dirties only its own AABB; removing a whole layer (crater/
/// rock) or an unknown id falls back to a whole-terrain re-bake. Reads the stack
/// BEFORE the removal observer runs (registered first), so the edit is still present.
fn on_remove_terrain_layer_dirty(
    trigger: On<RemoveTerrainLayer>,
    mut commands: Commands,
    mut q: Query<
        (Entity, &crate::terrain_layers::TerrainLayerStack, Option<&mut TerrainDirty>),
        With<DemBaseGrid>,
    >,
) {
    let id = crate::terrain_layers::LayerId::new(trigger.event().id.clone());
    for (e, stack, dirty) in q.iter_mut() {
        let mark = stack.edit_bounds(&id); // Some = a bounded edit; None = whole layer / absent
        match (dirty, mark) {
            (Some(mut d), Some(aabb)) => d.grow(aabb),
            (Some(mut d), None) => d.bounds = None,
            (None, m) => {
                commands.entity(e).try_insert(TerrainDirty { bounds: m });
            }
        }
    }
}

/// A live crater/rock spec change re-bakes the WHOLE terrain, so mark it as such.
fn on_obstacle_spec_dirty(
    _trigger: On<lunco_obstacle_field::plugin::UpdateObstacleFieldSpec>,
    mut commands: Commands,
    mut q: Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
) {
    accumulate_terrain_dirty(&mut commands, &mut q, None);
}

/// Register the DEM-terrain command + spawn systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.register_type::<SpawnDemTerrain>()
        .init_resource::<TerrainEditSeq>()
        .init_resource::<crate::stream_viz::LodMeshCache>()
        .add_message::<RegenerateTerrainLayers>()
        .add_observer(on_obstacle_spec_rebuild_layers)
        // Dirty-region markers — registered BEFORE the command observers (below) so a
        // remove reads the edit's bounds before the removal applies.
        .add_observer(on_brush_terrain_dirty)
        .add_observer(on_flatten_terrain_dirty)
        .add_observer(on_remove_terrain_layer_dirty)
        .add_observer(on_obstacle_spec_dirty)
        .add_systems(
            Update,
            (
                start_dem_builds,
                finish_dem_builds,
                start_dem_restamp,
                finish_dem_restamp,
                start_dem_collider,
                finish_dem_collider,
            ),
        );
    register_all_commands(app);
}
