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

// The pure decode → crop/resample pipeline lives in the bevy/avian-free
// `lunco-terrain-bake` crate, so the wasm Web Worker (`dem_worker`) offloads the
// heavy GeoTIFF decode; the analytic oracle composition + avian collider + Bevy
// mesh derive stay here (on the main thread on web — they're cheap).
use lunco_terrain_bake::bake::{crop_centered, resample};
use lunco_terrain_bake::dem::height_grid_from_geotiff;
#[cfg(target_arch = "wasm32")]
use lunco_terrain_bake::{BakedGrid, DemBakeJob};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// Default realized region side length (metres) when `window_m` is 0… no — see
/// below: 0 means the whole map. This is the fallback when a caller passes a
/// negative value. A 4 km window at 5 m is 800² ≈ 640 k verts — full detail,
/// light to render, and covers a rover working area.
const DEFAULT_WINDOW_M: f32 = 4096.0;
/// Above this native tile resolution we still build, but warn: a single mesh this
/// large is heavy (e.g. the full 16 km map is 3200² ≈ 10 M verts ≈ 560 MB). Full
/// detail at that scale belongs to tiled streaming (M7), not one mesh.
const HEAVY_TILE_RES: usize = 2048;
/// Largest `window_m` a `SpawnDemTerrain` command may ask for (metres). Bigger
/// than any shipped DEM site (16 km) with headroom; a wild value from the API /
/// rhai would otherwise crop far past the raster for no gain.
const MAX_WINDOW_M: f32 = 64_000.0;
/// Bounds on `SpawnDemTerrain::target_res` (samples per side). The value goes
/// straight into a `res × res` vertex grid, so an unclamped `100000` from a
/// command payload is a 10-billion-vertex allocation — an instant OOM. The crater
/// COUNT is clamped the same way (`crater_placements`, 250 k); this applies the
/// same pattern to the command's own params. `MAX` is 2× `HEAVY_TILE_RES`
/// (4096² ≈ 16.8 M verts) — heavy, warned about, but survivable.
const MIN_TARGET_RES: u32 = 16;
const MAX_TARGET_RES: u32 = 4096;

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
    // Authored `density` (per ha) is calibrated to the legacy 8 m-radius size
    // floor. The power-law SFD `N(>r) ∝ r^-1.8` says lowering the floor to
    // `size.min` multiplies the population by `(8/min)^1.8` — that growth is the
    // saturation-equilibrium small-crater carpet, not a density change, so scale
    // the count here instead of asking every scene to re-author density. Capped:
    // the analytic index stays cheap per-sample, but a pathological min would
    // otherwise mint millions of placements.
    #[cfg(target_arch = "wasm32")]
    let rmin = (craters.size.min as f64).max(1.5);
    #[cfg(not(target_arch = "wasm32"))]
    let rmin = (craters.size.min as f64).max(0.5);

    let sfd_scale = (8.0 / rmin).powf(1.8).max(1.0);
    let count = ((craters.density as f64 * side * side) / 10_000.0 * sfd_scale)
        .round()
        .max(0.0)
        .min(250_000.0) as usize;
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
    /// DEM site directory (contains `materials/textures/heightmap.tif`).
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
pub struct DemBaseGrid(
    pub std::sync::Arc<HeightGrid>,
    /// Cached `grid_key` of the base grid (folds the whole raster). Computed once
    /// at build time so a live re-stamp reusing this Arc never re-hashes the
    /// multi-million-point grid — the all-analytic stack (the norm) reuses it.
    pub u64,
);

/// Retained build settings a live re-bake needs (whether the static collider or a
/// streamed collider ring carries physics).
#[derive(Component, Clone, Copy)]
pub struct DemTerrainSource {
    pub collider_ring: bool,
}

/// Live, human-readable progress of DEM terrain generation — the data source for
/// a "Generating terrain…" loading overlay. Kept in this crate (not the UI) so
/// every host app shares one source of truth with no egui dependency here; the
/// sandbox reads it each frame and paints a centered card while `active`.
///
/// Progress is derived from build-component presence each frame ([`update_terrain_gen_status`]),
/// so it needs no plumbing through the async bake tasks. The heavy crater stamp
/// exposes no incremental callback, so `fraction` stays `None` (the UI shows an
/// animated spinner) — `phase` carries the meaningful signal.
/// The current terrain-build phase — a **typed** signal so the UI caption can't
/// drift from a stringly-matched substring (native "Baking"/"Preparing" silently
/// fell through the old `phase.contains("…")` match to the wrong caption).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TerrainGenPhase {
    /// Queued, no bake started yet.
    #[default]
    Preparing,
    /// Web: fetching the GeoTIFF from the server.
    Downloading,
    /// Web: baking the coarse preview (decode + stamp + collider).
    Building,
    /// Native: the single async decode + crater-stamp bake.
    Baking,
    /// Web: refining the full-resolution grid after the coarse preview.
    Refining,
}

impl TerrainGenPhase {
    /// Short label for the overlay title (e.g. `"Refining terrain"`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing",
            Self::Downloading => "Downloading terrain",
            Self::Building => "Building terrain",
            Self::Baking => "Baking terrain",
            Self::Refining => "Refining terrain",
        }
    }

    /// One-line caption describing what the phase is doing (overlay subtext).
    pub fn caption(self) -> &'static str {
        match self {
            Self::Preparing => "Preparing terrain generation…",
            Self::Downloading => "Fetching heightmap file from server…",
            // Native "Baking" and web "Building" both decode + stamp + build colliders.
            Self::Building | Self::Baking => {
                "Decoding heightmap, stamping craters, building colliders…"
            }
            Self::Refining => "Refining high-detail meshes and colliders near camera…",
        }
    }
}

#[derive(Resource, Default, Clone)]
pub struct TerrainGenStatus {
    /// A build is in flight — any tile queued, baking (native), or streaming (web).
    pub active: bool,
    /// Site label (last path segment of the DEM uri, e.g. `connecting_ridge`).
    pub site: String,
    /// Current phase (drives the overlay title + caption).
    pub phase: TerrainGenPhase,
    /// `0.0..=1.0` when a real fraction is known; `None` = indeterminate (spinner).
    pub fraction: Option<f32>,
    /// Set to true if the user dismissed the loading overlay manually.
    pub user_dismissed: bool,
}

/// Safety valve: if a build's components linger this long (a lost worker reply,
/// a wedged bake) clear the overlay so the UI isn't blocked forever. Matches the
/// spirit of the 30 s ground-collider gate — degrade to "done" loudly rather than
/// freeze the screen behind a spinner.
const GEN_STATUS_MAX_SECS: f32 = 60.0;

/// Derive [`TerrainGenStatus`] from the terrain build components each frame. A
/// tile is "generating" while it carries a [`DemTerrainRequest`] (queued or
/// mid-native-bake) or a [`DemWorkerJob`] (web worker decode/stream, after the
/// request is consumed). Runs every frame so the overlay clears the instant the
/// last tile finishes.
///
/// On web the bake is a coarse-preview → full-refine pair, so we map component
/// state to a real fraction: `Preparing` → `Building` (coarse) → `Refining`
/// (full). Native is one async task with no incremental signal → indeterminate.
fn update_terrain_gen_status(
    time: Res<Time>,
    // `has_request` distinguishes web's two worker phases: the request is dropped
    // once the coarse grid lands, so `job && !request` = refining the full grid.
    requests: Query<(&DemTerrainRequest, Has<DemBuildTask>)>,
    worker_jobs: Query<(Option<&DemTerrainRequest>, &DemWorkerJob)>,
    mut status: ResMut<TerrainGenStatus>,
    mut elapsed: Local<f32>,
) {
    let mut baking = false; // native async bake task in flight
    let mut queued = false; // a request exists (queued or mid-bake)
    let mut site: Option<&str> = None;
    for (req, has_task) in &requests {
        queued = true;
        baking |= has_task;
        if site.is_none() {
            site = req.uri.rsplit(['/', '\\']).next().filter(|s| !s.is_empty());
        }
    }
    // Web worker phase: coarse still pending (request present) vs full refine.
    #[allow(unused_mut)]
    let mut download_fraction: Option<f32> = None;
    let mut streaming_coarse = false;
    let mut refining_full = false;
    for (req, job) in &worker_jobs {
        let _ = job;
        if req.is_some() {
            streaming_coarse = true;
            #[cfg(target_arch = "wasm32")]
            {
                if let Ok(guard) = job.download_progress.lock() {
                    if let Some((done, total)) = *guard {
                        if total > 0 {
                            download_fraction = Some(done as f32 / total as f32);
                        }
                    }
                }
            }
        } else {
            refining_full = true;
        }
    }
    // Only show the giant centered loader overlay during preparation, download,
    // and coarse build (before the first mesh/collider lands). Background
    // refinement runs silently in the background, updating tiles dynamically.
    let active = queued || streaming_coarse;

    if !active {
        if status.active || status.user_dismissed {
            *status = TerrainGenStatus::default();
        }
        *elapsed = 0.0;
        return;
    }

    if status.user_dismissed {
        status.active = false;
        return;
    }

    *elapsed += time.delta_secs();
    if *elapsed > GEN_STATUS_MAX_SECS {
        if status.active {
            warn!(
                "[terrain] generation overlay held {GEN_STATUS_MAX_SECS}s — clearing \
                 (lost worker reply or stuck bake?)"
            );
            *status = TerrainGenStatus::default();
        }
        return;
    }

    status.active = true;
    if let Some(s) = site {
        status.site = s.to_string();
    }
    // Phase + a coarse→full fraction where the pipeline exposes one. Web streams a
    // coarse preview (physics-ready) then refines to the full grid; native is a
    // single opaque task (indeterminate spinner).
    let (phase, fraction) = if refining_full {
        (TerrainGenPhase::Refining, Some(0.66))
    } else if streaming_coarse {
        if let Some(df) = download_fraction {
            if df < 1.0 {
                (TerrainGenPhase::Downloading, Some(0.1 + df * 0.23))
            } else {
                (TerrainGenPhase::Building, Some(0.33))
            }
        } else {
            (TerrainGenPhase::Building, Some(0.33))
        }
    } else if baking {
        (TerrainGenPhase::Baking, None)
    } else {
        (TerrainGenPhase::Preparing, Some(0.1))
    };
    status.phase = phase;
    status.fraction = fraction;
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
        w if !w.is_finite() => (DEFAULT_WINDOW_M * 0.5) as f64, // NaN/inf → default
        w if w == 0.0 => f64::INFINITY,          // whole map
        w if w < 0.0 => (DEFAULT_WINDOW_M * 0.5) as f64,
        w => (w.min(MAX_WINDOW_M) * 0.5) as f64,
    };
    // Clamp the resample target the same way the crater count is clamped
    // (`crater_placements`): `target_res` lands straight in a `res × res` vertex
    // grid, so an unclamped `100000` from the API/rhai is an instant OOM. `0` =
    // native (no decimation).
    let target_res = match ev.target_res {
        0 => 0,
        r => (r.clamp(MIN_TARGET_RES, MAX_TARGET_RES)) as usize,
    };
    if ev.target_res != 0 && target_res != ev.target_res as usize {
        warn!(
            "[dem-terrain] target_res {} out of range — clamped to {}",
            ev.target_res, target_res
        );
    }
    // Standalone entity, anchored into the world grid at the origin cell (when it
    // exists). The USD path instead places `DemTerrainRequest` on the prim entity,
    // which already carries its USD transform + grid parentage.
    let mut e = commands.spawn((
        DemTerrainSurface,
        Name::new("DemTerrain"),
        DemTerrainRequest {
            uri: ev.uri.clone(),
            half_window,
            target_res,
            lod_viz: ev.lod_viz,
            collider_ring: ev.collider_ring,
            with_default_material: true,
        },
        stack,
        Transform::IDENTITY,
        Visibility::Inherited,
    ));
    if let Ok(grid) = grids.single() {
        e.try_insert((CellCoord::default(), GridAnchor, ChildOf(grid)));
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

/// Place ONE hand-authored impact crater: rim radius `radius` m centred at
/// terrain-local `(x, z)`, bowl `depth` m (0 = realistic default `0.4·radius`,
/// the fresh d/D ≈ 0.2 morphology). Same analytic profile as the procedural
/// field, so it lands in mesh + collider + derived maps alike, and it is an
/// addressable edit — remove it later via `RemoveTerrainLayer{id}`. Reachable
/// as `cmd("PlaceCrater", #{x, z, radius})`.
#[Command(default)]
pub struct PlaceCrater {
    pub x: f32,
    pub z: f32,
    pub radius: f32,
    /// Bowl depth in metres; 0/absent = realistic default (0.4·radius).
    pub depth: f32,
    /// Optional stable id for the edit (so it can be removed later). Empty = auto.
    pub id: String,
}

impl PlaceCrater {
    /// The effective bowl depth: authored, or the fresh-crater default.
    pub fn depth_or_default(&self) -> f64 {
        if self.depth > 0.0 {
            self.depth as f64
        } else {
            0.4 * self.radius as f64
        }
    }
}

#[on_command(PlaceCrater)]
fn on_place_crater(
    trigger: On<PlaceCrater>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
    mut seq: ResMut<TerrainEditSeq>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let id = edit_id(&ev.id, "crater", &mut seq);
    let kind = crate::terrain_layers::EditKind::Crater {
        center: [ev.x as f64, ev.z as f64],
        radius: ev.radius as f64,
        depth: ev.depth_or_default(),
    };
    for mut stack in &mut terrains {
        stack.add_edit(id.clone(), kind);
    }
}

/// Place ONE hand-authored boulder at terrain-local `(x, z)`, radius `size` m —
/// its own addressable layer (removable via `RemoveTerrainLayer{id}`). Same
/// mesh/collider derivation as the procedural rock field, so it looks and
/// drives identically. Reachable as `cmd("PlaceRock", #{x, z, size})`.
#[Command(default)]
pub struct PlaceRock {
    pub x: f32,
    pub z: f32,
    /// Boulder radius in metres; 0/absent = 0.6 m.
    pub size: f32,
    /// Shape/orientation seed; 0 = derived from position (stable, varied).
    pub seed: u64,
    /// Optional stable id for the layer (so it can be removed later). Empty = auto.
    pub id: String,
}

impl PlaceRock {
    /// Effective boulder radius (default 0.6 m, clamped to sane bounds).
    pub fn size_or_default(&self) -> f32 {
        if self.size > 0.0 {
            self.size.clamp(0.05, 10.0)
        } else {
            0.6
        }
    }

    /// Effective shape seed: authored, or a stable hash of the position so two
    /// placed rocks never share facets/yaw by default.
    pub fn seed_or_default(&self) -> u64 {
        if self.seed != 0 {
            return self.seed;
        }
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64((self.x as f64).to_bits());
        h.write_u64((self.z as f64).to_bits());
        h.finish() | 1
    }
}

#[on_command(PlaceRock)]
fn on_place_rock(
    trigger: On<PlaceRock>,
    mut terrains: Query<&mut crate::terrain_layers::TerrainLayerStack, Without<DocBackedTerrain>>,
    mut seq: ResMut<TerrainEditSeq>,
) {
    let ev = trigger.event();
    let id = edit_id(&ev.id, "rock", &mut seq);
    let layer = crate::terrain_layers::rock_instance_layer(
        [ev.x as f64, ev.z as f64],
        ev.size_or_default(),
        ev.seed_or_default(),
    );
    for mut stack in &mut terrains {
        stack.push_layer(id.clone(), layer.clone());
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
    on_place_crater,
    on_place_rock,
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
    /// `grid_key(&base_grid)`, computed where the grid was produced (off-thread
    /// on native) — assembly must never re-fold the multi-million-point raster
    /// on the main thread.
    base_key: u64,
    half_extent: f32,
    /// Tile resolution actually meshed (= native crop res, or the resample target).
    res: usize,
    /// Native crop resolution before any resample (for honest logging).
    native_res: usize,
    site: String,
}

#[derive(Component)]
struct DemBuildTask(Task<Result<DemBuild, String>>);

/// WEB: marks a terrain whose bake was dispatched to the off-thread DEM worker
/// (in place of a `DemBuildTask`). Carries the assembly settings the worker's
/// replies need — the request marker is dropped on the coarse reply (to release
/// the physics hold), so these can't be read from it later. `id` (from
/// [`dem_worker_job_id`]) correlates a reply back to this entity.
#[derive(Component, Clone)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub struct DemWorkerJob {
    id: u32,
    collider_ring: bool,
    lod_viz: bool,
    with_default_material: bool,
    #[cfg(target_arch = "wasm32")]
    pub download_progress: std::sync::Arc<std::sync::Mutex<Option<(u64, u64)>>>,
    /// OPFS grid-cache key (hex), filled by the fetch task on a cache MISS so the
    /// Full worker reply writes the baked grid through; stays `None` on a hit
    /// (nothing to rewrite). Slot-shared like `download_progress` because the key
    /// derives from the fetched bytes, which only the detached task sees.
    #[cfg(target_arch = "wasm32")]
    cache_key: std::sync::Arc<std::sync::Mutex<Option<String>>>,
}

/// OPFS namespace for the Full-stage baked DEM grid (web cold-start cache).
#[cfg(target_arch = "wasm32")]
const DEM_GRID_CACHE_NS: &str = "terrain/dem-grid";

/// Wire id for a [`DemWorkerJob`]: the entity's index XOR-folded with its
/// generation. The bare index is NOT an identity — Bevy reuses indices after a
/// despawn, and a stale reply for a mid-bake-despawned terrain must never land
/// on an unrelated entity that inherited its index.
#[cfg(target_arch = "wasm32")]
fn dem_worker_job_id(entity: Entity) -> u32 {
    let bits = entity.to_bits();
    (bits ^ (bits >> 32)) as u32
}

/// Retire a despawned terrain's in-flight worker bake. Without this the id stays
/// in the worker client's in-flight set for the session, and a later
/// worker-crash sweep emits a spurious error reply for an entity long gone.
#[cfg(target_arch = "wasm32")]
fn cancel_worker_job_on_remove(trigger: On<Remove, DemWorkerJob>, jobs: Query<&DemWorkerJob>) {
    if let Ok(job) = jobs.get(trigger.entity) {
        lunco_terrain_bake::worker_client::cancel(job.id);
    }
}

/// Compose the surface oracle (raster base + the layers' ANALYTIC height modifiers)
/// and derive the avian collider + Bevy `MeshData` a pure [`BakedGrid`] needs into a
/// [`DemBuild`], ready for [`assemble_dem_build`]. Used by the web worker path: the
/// worker offloads the heavy GeoTIFF decode and returns the BARE grid, then this
/// runs on the main thread — the analytic composition is cheap, so web keeps the
/// SAME unbounded crater/edit realism as native. Streaming (`collider_ring` +
/// `lod_viz`) builds neither a static collider nor a static mesh (the ring / tiles
/// sample the oracle directly), so it doesn't even materialize.
#[cfg(target_arch = "wasm32")]
fn dem_build_from_baked(
    baked: BakedGrid,
    collider_ring: bool,
    lod_viz: bool,
    contributions: Vec<crate::oracle::HeightContribution>,
) -> DemBuild {
    let half_extent = baked.grid.half_extent;
    let base_grid = std::sync::Arc::new(baked.base_grid);
    // The worker's `base_grid` IS the (stamped) working grid, so ONE fold keys
    // both the retained base and the oracle — this runs on the wasm main
    // thread, where a second multi-million-point fold is a visible hitch.
    let base_key = crate::oracle::grid_key(&base_grid);
    let oracle = std::sync::Arc::new(crate::oracle::SurfaceOracle::new_with_base_key(
        std::sync::Arc::new(baked.grid),
        contributions,
        base_key,
    ));
    // Static consumers rasterise the composed surface once; streaming consumers
    // sample the oracle directly (so no materialize for the ring/tile path).
    let needs_static_grid = !collider_ring || !lod_viz;
    let materialized = needs_static_grid.then(|| oracle.materialize());
    let collider = if collider_ring {
        None
    } else {
        let g = materialized.as_ref().expect("materialized for static collider");
        let h = g.half_extent as f64;
        Some(Collider::heightfield(g.to_avian_heights(), DVec3::new(2.0 * h, 1.0, 2.0 * h)))
    };
    let mesh = if lod_viz { None } else { materialized.as_ref().map(|g| g.to_mesh_data()) };
    DemBuild {
        collider,
        mesh,
        oracle: Some(oracle),
        base_grid,
        base_key,
        half_extent,
        res: baked.res,
        native_res: baked.native_res,
        site: baked.site,
    }
}

/// The analytic height contributions for a terrain's layer stack — the SAME
/// composition the native build task does inline, factored out so the web worker's
/// replies ([`finish_dem_worker`]) can rebuild the oracle from the entity's stack.
#[cfg(target_arch = "wasm32")]
fn layer_contributions(
    stack: Option<&crate::terrain_layers::TerrainLayerStack>,
    half_extent: f32,
    curvature_radius: Option<f64>,
) -> Vec<crate::oracle::HeightContribution> {
    let mut contributions: Vec<_> = stack
        .map(|s| {
            s.0.iter()
                .filter(|e| e.layer.stamp_spec().is_none())
                .filter_map(|e| e.layer.height_modifier(half_extent))
                .collect()
        })
        .unwrap_or_default();
    if let Some(r) = curvature_radius {
        contributions.push(crate::oracle::curvature_contribution(r, half_extent));
    }
    contributions
}

/// Read a DEM file's bytes through the platform's I/O.
///
/// Native reads through `lunco-storage::FileStorage`; wasm fetches same-origin
/// over HTTP (the Twin's terrain folder is staged next to the wasm under
/// `assets/`), cached in a Cache-Storage bucket so the large heightmap
/// re-hydrates instantly next load, with a background *conditional*
/// revalidation so a host-side DEM replacement is picked up on the following
/// reload. Pure I/O (an `await`, not CPU) → safe to run on the main-thread
/// event loop; the heavy decode/stamp is what moves to the worker.
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
        use lunco_storage::{OpfsStorage, StorageHandle};
        // Two DEM origins on web, disambiguated by where the file actually lives:
        //  • a **scenario-sync twin** — fetched over the network into the OPFS
        //    scenario cache `<cache>/scenarios/<id>/…`, which is mounted as that
        //    Twin's root, so the DEM source resolves to an absolute path under
        //    that cache dir. Read it straight from OPFS.
        //  • the **baked-in demo twin** — staged next to the wasm under `assets/`,
        //    fetched same-origin over HTTP, cached in Cache-Storage. Those URLs
        //    are MUTABLE — a host-side twin update can replace the file in place —
        //    so the cached copy is served now and a background
        //    `If-None-Match`/`If-Modified-Since` probe refreshes the cache for the
        //    next reload.
        // The baked demo's `twins/…` path never exists under `scenarios/`, so an
        // OPFS existence check cleanly picks the right backend.
        let opfs_candidate = lunco_assets::scenarios_dir().join(&path);
        let opfs = OpfsStorage::new();
        if opfs.exists(&StorageHandle::File(opfs_candidate.clone())).await {
            return opfs
                .read(&StorageHandle::File(opfs_candidate))
                .await
                .map_err(|e| e.to_string());
        }
        let url = lunco_assets::asset_path::web_url(&path.to_string_lossy());
        lunco_assets::web_fetch::fetch_bytes_cached_conditional("lunco-twin-v1", &url).await
    }
}

#[cfg(target_arch = "wasm32")]
static WASM_BAKE_FAILURES_TX: std::sync::OnceLock<std::sync::mpsc::Sender<u32>> = std::sync::OnceLock::new();
#[cfg(target_arch = "wasm32")]
static WASM_BAKE_FAILURES_RX: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<u32>>> = std::sync::OnceLock::new();

#[cfg(target_arch = "wasm32")]
fn get_wasm_bake_failures_tx() -> &'static std::sync::mpsc::Sender<u32> {
    WASM_BAKE_FAILURES_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = WASM_BAKE_FAILURES_RX.set(std::sync::Mutex::new(rx));
        tx
    })
}

#[cfg(target_arch = "wasm32")]
fn get_wasm_bake_failures_rx() -> &'static std::sync::Mutex<std::sync::mpsc::Receiver<u32>> {
    let _ = get_wasm_bake_failures_tx(); // ensures initialized
    WASM_BAKE_FAILURES_RX.get().unwrap()
}

/// Kick a build for each pending request. On native (and the web fallback if no
/// worker was staged) the shared `bake_grid` runs in an `AsyncComputeTaskPool`
/// task; on web it is dispatched to the off-thread [`dem_worker`] so the decode +
/// crater stamp never freezes the page. Both paths run the SAME bake code.
fn start_dem_builds(
    mut commands: Commands,
    q: Query<
        (Entity, &DemTerrainRequest, Option<&crate::terrain_layers::TerrainLayerStack>),
        (Without<DemBuildTask>, Without<DemWorkerJob>),
    >,
    curvature: Option<Res<crate::oracle::TerrainBodyCurvature>>,
) {
    // Parent-body radius for site-anchored scenes — folded LAST over the layer
    // stack so the tangent-plane DEM hugs the body sphere. (If the celestial
    // anchor lands AFTER this build captured `None`, `restamp_on_curvature`
    // re-composes the oracle with it.)
    let curvature_radius = curvature.map(|c| c.radius_m);
    for (entity, req, stack) in &q {
        let dir = std::path::PathBuf::from(&req.uri);
        let tif_path = dir.join("materials/textures/heightmap.tif");
        // Site identity = the DEM folder name (`terrain/apollo15/` → "apollo15").
        // This is the ONE fact the raster genuinely does not carry, and the folder
        // already states it — so it needs no sidecar either. It keys the bake cache
        // and the streamed-tile store.
        let site_id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("site")
            .to_string();
        let collider_ring = req.collider_ring;
        let lod_viz = req.lod_viz;
        // Captured by-value into the 'static async build task (the query item `req`
        // can't cross the task boundary).
        let half_window = req.half_window;
        let target_res = req.target_res;
        // The terrain's composed layer stack (from its USD child layer prims).
        // Height layers (craters, edits) contribute ANALYTIC modifiers to the
        // surface oracle: composed OFF-THREAD in the native build task below, and
        // on the main thread from the SAME stack after the wasm worker returns its
        // bare grid — so web keeps full analytic realism (see `finish_dem_worker`).
        // Scatter layers (rocks) are no-ops here; they run post-build.
        let layers: Vec<_> =
            stack.map(|s| s.0.iter().map(|e| e.layer.clone()).collect()).unwrap_or_default();
        // WASM: the DEM worker offloads GeoTIFF decode + crop + resample + static
        // stamps (craters). Only non-stamped layers (like live drawing) compose
        // analytically on the main thread.
        #[cfg(target_arch = "wasm32")]
        let stamps: Vec<_> =
            layers.iter().filter_map(|l| l.stamp_spec()).collect();
        #[cfg(target_arch = "wasm32")]
        let job = DemBakeJob {
            half_window: req.half_window,
            target_res: req.target_res,
            detail_upsample: 1,
            stamps,
        };

        // WEB: offload decode + stamp to the DEM Web Worker. A detached I/O task
        // fetches the bytes (an `await`, not a freeze) then hands them to the worker,
        // which streams a coarse preview and then the full grid back to
        // `finish_dem_worker`. The heavy CPU never touches the page's main thread.
        #[cfg(target_arch = "wasm32")]
        if lunco_terrain_bake::worker_client::is_available() {
            let id = dem_worker_job_id(entity);
            let download_progress = std::sync::Arc::new(std::sync::Mutex::new(None));
            let cache_key = std::sync::Arc::new(std::sync::Mutex::new(None));
            commands.entity(entity).insert(DemWorkerJob {
                id,
                collider_ring,
                lod_viz,
                with_default_material: req.with_default_material,
                download_progress: download_progress.clone(),
                cache_key: cache_key.clone(),
            });
            let job = job.clone();
            AsyncComputeTaskPool::get()
                .spawn(async move {
                    let tx = get_wasm_bake_failures_tx().clone();
                    
                    // A scenario-synced twin's DEM already lives in the OPFS scenario
                    // cache — read it from there rather than re-fetching it over HTTP
                    // (where it doesn't exist: `assets/<scenario-id>/…` is a 404).
                    // Only the baked-in demo twin is staged under `assets/`. This
                    // mirrors `read_bytes`; the .tif can't reuse it because it
                    // wants progress reporting.
                    let opfs_tif = lunco_assets::scenarios_dir().join(&tif_path);
                    let opfs_handle = lunco_storage::StorageHandle::File(opfs_tif);
                    let opfs = lunco_storage::OpfsStorage::new();
                    let tif = if opfs.exists(&opfs_handle).await {
                        // Local read: no network, so report it as instantly complete
                        // rather than leaving the progress bar at zero.
                        match opfs.read(&opfs_handle).await {
                            Ok(b) => {
                                if let Ok(mut s) = download_progress.lock() {
                                    let n = b.len() as u64;
                                    *s = Some((n, n));
                                }
                                b
                            }
                            Err(e) => {
                                bevy::log::error!("[dem-terrain] tif read from scenario cache failed: {e}");
                                let _ = tx.send(id);
                                return;
                            }
                        }
                    } else {
                        let url = lunco_assets::asset_path::web_url(&tif_path.to_string_lossy());

                        let progress_slot = download_progress.clone();
                        let progress_cb = wasm_bindgen::closure::Closure::<dyn FnMut(f64, f64)>::new(move |done: f64, total: f64| {
                            if let Ok(mut s) = progress_slot.lock() {
                                *s = Some((done as u64, total as u64));
                            }
                        });

                        // Cached-first with a download bar, plus a background
                        // conditional revalidate: the heightmap URL is mutable (a
                        // host-side twin update replaces it in place), so a changed
                        // DEM is re-cached for the next reload instead of being
                        // served stale forever.
                        let fetched = lunco_assets::web_fetch::fetch_cached_with_progress_conditional(
                            "lunco-twin-v1",
                            &url,
                            0,
                            progress_cb.as_ref().unchecked_ref(),
                        )
                        .await;
                        drop(progress_cb);
                        match fetched {
                            Ok(b) => b,
                            Err(e) => {
                                bevy::log::error!("[dem-terrain] tif fetch failed: {e}");
                                let _ = tx.send(id);
                                return;
                            }
                        }
                    };

                    // OPFS grid cache: key = format version + the RAW fetched
                    // bytes + the job params (content-exact — composes with the
                    // conditional-revalidation fetch above: a replaced DEM is new
                    // tif bytes, hence a new key, hence a rebake). On a hit the
                    // worker is never spawned; the grid is injected through the
                    // SAME reply queue a Full worker reply uses.
                    let key =
                        lunco_terrain_bake::grid_cache_key(site_id.as_bytes(), &tif, &job);
                    let key_hex = lunco_precompute::key_hex(key);
                    if let Some(blob) =
                        lunco_storage::opfs_blob::read(DEM_GRID_CACHE_NS, &key_hex).await
                    {
                        if let Some((grid, native_res)) =
                            lunco_terrain_bake::decode_grid_blob(&blob)
                        {
                            let site = site_id.clone();
                            bevy::log::info!(
                                "[dem-terrain] OPFS grid-cache hit ({}², key {key_hex}) — skipping worker bake",
                                grid.res
                            );
                            lunco_terrain_bake::worker_client::push_local_reply(
                                lunco_terrain_bake::worker_client::WorkerReply {
                                    id,
                                    stage: lunco_terrain_bake::BakeStage::Full,
                                    site,
                                    native_res,
                                    res: grid.res,
                                    grid: Ok(grid),
                                },
                            );
                            return;
                        }
                    }
                    // Miss: remember the key so the Full reply persists the grid.
                    if let Ok(mut slot) = cache_key.lock() {
                        *slot = Some(key_hex);
                    }

                    if let Err(e) =
                        lunco_terrain_bake::worker_client::dispatch(id, &job, &site_id, &tif)
                    {
                        bevy::log::error!("[dem-terrain] worker dispatch failed: {e:?}");
                        let _ = tx.send(id);
                    }
                })
                .detach();
            continue;
        }

        // NATIVE (and web without a staged worker): full bake in an async task — a
        // real thread on native. Decode the DEM and compose the analytic surface
        // oracle here (crater/edit modifiers unbounded by grid res); the avian
        // collider + Bevy mesh derive is added off-thread too.
        let task = AsyncComputeTaskPool::get().spawn(async move {
            // ONE file. The `metadata.yaml` read that used to precede this is gone:
            // the raster states its own extent and position, so there is no second
            // document to fetch, parse, or disagree with.
            let tif = read_bytes(tif_path).await?;
            let grid = height_grid_from_geotiff(&tif).map_err(|e| e.to_string())?;

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
            // it without re-reading the GeoTIFF. Its content key is folded HERE,
            // off-thread — assembly on the main thread reuses it.
            let base_grid = std::sync::Arc::new(tile.clone());
            let base_key = crate::oracle::grid_key(&base_grid);
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
            let mut contributions: Vec<_> =
                layers.iter().filter_map(|l| l.height_modifier(half_extent)).collect();
            if let Some(r) = curvature_radius {
                contributions.push(crate::oracle::curvature_contribution(r, half_extent));
            }
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
                base_key,
                half_extent,
                res,
                native_res,
                site: site_id,
            })
        });
        commands.entity(entity).try_insert(DemBuildTask(task));
    }
}

/// Collect finished native builds and fill the requesting entity (shared assembly
/// with the web worker path via [`assemble_dem_build`]).
fn finish_dem_builds(
    mut commands: Commands,
    mut tasks: Query<(Entity, &mut DemBuildTask, &DemTerrainRequest)>,
    // Optional so the headless server (no render assets) still builds colliders.
    //
    // There is no `Assets<StandardMaterial>` here any more: the default surface is
    // stated as `lunco_render::PbrLook` INTENT and bound by `lunco-render-bevy`, so
    // this crate names no material and links no `bevy_pbr`.
    mut meshes: Option<ResMut<Assets<Mesh>>>,
) {
    use bevy::tasks::futures_lite::future;

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
        assemble_dem_build(
            &mut commands,
            entity,
            req.collider_ring,
            req.lod_viz,
            req.with_default_material,
            built,
            meshes.as_deref_mut(),
        );
    }
}

/// Fill the terrain entity from a finished [`DemBuild`] — the shared assembly used
/// by BOTH `finish_dem_builds` (native task) and `finish_dem_worker` (web worker
/// coarse reply). Colliders spawn always (headless physics parity); the static
/// visual mesh only when render assets exist and `lod_viz` isn't streaming tiles.
#[allow(clippy::too_many_arguments)]
fn assemble_dem_build(
    commands: &mut Commands,
    entity: Entity,
    collider_ring: bool,
    lod_viz: bool,
    with_default_material: bool,
    built: DemBuild,
    meshes: Option<&mut Assets<Mesh>>,
) {
    if built.res > HEAVY_TILE_RES {
        warn!(
            "[dem-terrain] '{}' tile is {}² verts — heavy for a single mesh; \
             tiled streaming + LOD (M7) is the path for full-map detail",
            built.site, built.res,
        );
    }
    let h = built.half_extent as f64;
    {
        let mut e = commands.entity(entity);
        // Static full-DEM collider (`None` when a collider ring streams per-tile
        // colliders instead) — already built (off-thread on native).
        if let Some(collider) = built.collider {
            e.try_insert((RigidBody::Static, collider));
        }
        // Retain the pristine base grid + source settings so the crater layer can be
        // re-baked live from the Inspector (`RegenerateField`) without disk I/O.
        e.try_insert((
            DemBaseGrid(built.base_grid, built.base_key),
            DemTerrainSource { collider_ring },
        ));
        // Retain the oracle + mark the streaming mode(s). `lod_viz` streams visual LOD
        // tiles (static mesh suppressed); `collider_ring` streams physics tiles
        // (static collider suppressed above). Both sample the retained `DemHeightField`.
        if let Some(oracle) = built.oracle {
            e.try_insert(crate::stream_viz::DemHeightField(oracle));
            if lod_viz {
                e.try_insert((
                    crate::stream_viz::TerrainLodViz::default(),
                    crate::stream_viz::LodTiles::default(),
                    crate::stream_viz::PendingTileBakes::default(),
                    crate::stream_viz::TerrainNodeErrors::default(),
                    // Default Lit; switchable live in the Inspector (Terrain Shader).
                    crate::stream_viz::TerrainShaderMode::default(),
                ));
            }
            if collider_ring {
                e.try_insert((
                    crate::collider_ring::TerrainColliderRing::default(),
                    crate::collider_ring::ColliderTiles::default(),
                    crate::collider_ring::PendingColliderBakes::default(),
                ));
            }
        }
    }
    if let (Some(meshes), Some(mesh)) = (meshes, built.mesh) {
        let MeshData { positions, normals, uvs, indices } = mesh;
        // STATIC mesh: `default()` usages keep the CPU copy — `lunco-environment`'s
        // horizon bake reads its positions back and rewrites its UVs. (The streamed
        // LOD tiles are RENDER_WORLD-only; see `stream_viz`.)
        let handle = meshes.add(lunco_obstacle_field::grid_mesh(
            positions,
            normals,
            uvs,
            indices,
            bevy::asset::RenderAssetUsages::default(),
        ));
        commands.entity(entity).try_insert(Mesh3d(handle));
        // Default surface only for the standalone command path; the USD path authors
        // its own via `materialType` (don't clobber it). Stated as INTENT —
        // `lunco-render-bevy` turns it into a `StandardMaterial`, and a headless
        // build simply keeps the data and binds nothing.
        if with_default_material {
            commands
                .entity(entity)
                .try_insert(lunco_render::PbrLook::matte(Color::srgb(0.30, 0.29, 0.27).into()));
        }
    }
    let mode = match (lod_viz, collider_ring) {
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

/// WEB: apply the DEM worker's replies. The **coarse** preview assembles the
/// terrain like a native build (terrain + collider ring appear, the physics hold
/// releases, rovers settle); the **full** grid then swaps in via the SAME live
/// re-stamp machinery [`finish_dem_restamp`] uses, so the tiles refine
/// near-camera-first with no despawn flash. Progressive, and no code duplicated.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn finish_dem_worker(
    mut commands: Commands,
    jobs: Query<(Entity, &DemWorkerJob)>,
    mut swap_q: Query<(
        &mut crate::stream_viz::DemHeightField,
        Option<&mut crate::stream_viz::LodTiles>,
        Option<&mut crate::stream_viz::PendingTileBakes>,
        Has<Mesh3d>,
    )>,
    scattered: Query<Entity, With<crate::terrain_layers::TerrainScatterEntity>>,
    // The authored layer stack per terrain — its analytic crater/edit modifiers are
    // re-composed onto the worker's bare grid so web keeps full analytic realism.
    stacks: Query<&crate::terrain_layers::TerrainLayerStack>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    // `materials: Assets<StandardMaterial>` is GONE (render decoupling): `assemble_dem_build`
    // no longer binds a material here — it states a `PbrLook` intent and `lunco-render-bevy`
    // does the GPU bind. `curvature` stays: it is simulation data (the body-curvature "globe
    // punch") that `layer_contributions` composes into the height field.
    curvature: Option<Res<crate::oracle::TerrainBodyCurvature>>,
) {
    let curvature_radius = curvature.map(|c| c.radius_m);
    // Drain failed wasm bakes:
    if let Ok(rx) = get_wasm_bake_failures_rx().try_lock() {
        while let Ok(failed_id) = rx.try_recv() {
            if let Some((entity, _)) = jobs.iter().find(|(_, j)| j.id == failed_id) {
                commands.entity(entity).remove::<(DemTerrainRequest, DemWorkerJob)>();
            }
        }
    }

    let replies = lunco_terrain_bake::worker_client::drain_replies();
    if replies.is_empty() {
        return;
    }
    let mut any_full = false;
    for reply in replies {
        let Some((entity, job)) = jobs.iter().find(|(_, j)| j.id == reply.id) else {
            continue;
        };
        match (reply.stage, reply.grid) {
            (lunco_terrain_bake::BakeStage::Coarse, Ok(grid)) => {
                let contributions = layer_contributions(stacks.get(entity).ok(), grid.half_extent, curvature_radius);
                let baked = BakedGrid {
                    base_grid: grid.clone(),
                    grid,
                    site: reply.site,
                    native_res: reply.native_res,
                    res: reply.res,
                    stage: lunco_terrain_bake::BakeStage::Coarse,
                };
                let built =
                    dem_build_from_baked(baked, job.collider_ring, job.lod_viz, contributions);
                // Drop the request so the physics hold releases (rovers settle on the
                // coarse collider). Keep DemWorkerJob to receive the full grid.
                commands.entity(entity).remove::<DemTerrainRequest>();
                assemble_dem_build(
                    &mut commands,
                    entity,
                    job.collider_ring,
                    job.lod_viz,
                    job.with_default_material,
                    built,
                    meshes.as_deref_mut(),
                );
            }
            (lunco_terrain_bake::BakeStage::Full, Ok(grid)) => {
                // Best-effort OPFS write-through so the NEXT page load skips the
                // worker (GeoTIFF decode + crater stamp) entirely. The key was
                // derived from the fetched bytes on the miss path; it's `None` on
                // a cache hit (nothing to rewrite).
                if let Some(key_hex) = job.cache_key.lock().ok().and_then(|k| k.clone()) {
                    let blob = lunco_terrain_bake::encode_grid_blob(&grid, reply.native_res);
                    wasm_bindgen_futures::spawn_local(async move {
                        lunco_storage::opfs_blob::write(DEM_GRID_CACHE_NS, &key_hex, &blob)
                            .await;
                    });
                }
                // Re-compose the analytic oracle on the worker's full bare grid.
                let contributions = layer_contributions(stacks.get(entity).ok(), grid.half_extent, curvature_radius);
                if let Ok((mut hf, tiles, pending, has_static_mesh)) = swap_q.get_mut(entity) {
                    let base = std::sync::Arc::new(grid);
                    // D9: the ONLY `DemBaseGrid` insert is `assemble_dem_build`, and on
                    // web that ran for the COARSE preview. Without re-inserting it here
                    // the retained base stays coarse, so the next brush stroke's
                    // `spawn_restamp_task` re-composes from the coarse grid and the
                    // terrain visibly REVERTS to preview heights after sculpting.
                    let base_key = crate::oracle::grid_key(&base);
                    commands
                        .entity(entity)
                        .try_insert(DemBaseGrid(base.clone(), base_key));
                    let oracle = std::sync::Arc::new(
                        crate::oracle::SurfaceOracle::new_with_base_key(
                            base,
                            contributions,
                            base_key,
                        ),
                    );
                    swap_terrain_grid(
                        &mut commands,
                        entity,
                        oracle,
                        job.collider_ring,
                        &mut hf,
                        tiles,
                        pending,
                        has_static_mesh,
                        meshes.as_deref_mut(),
                    );
                    any_full = true;
                } else {
                    // The coarse reply landed THIS frame too, so its `DemHeightField`
                    // insert hasn't flushed yet and there's nothing to swap — assemble
                    // directly from the full grid instead (never drop the refined result).
                    let baked = BakedGrid {
                        base_grid: grid.clone(),
                        grid,
                        site: reply.site,
                        native_res: reply.native_res,
                        res: reply.res,
                        stage: lunco_terrain_bake::BakeStage::Full,
                    };
                    let built =
                        dem_build_from_baked(baked, job.collider_ring, job.lod_viz, contributions);
                    commands.entity(entity).remove::<DemTerrainRequest>();
                    assemble_dem_build(
                        &mut commands,
                        entity,
                        job.collider_ring,
                        job.lod_viz,
                        job.with_default_material,
                        built,
                        meshes.as_deref_mut(),
                    );
                }
                commands.entity(entity).remove::<DemWorkerJob>();
            }
            (stage, Err(e)) => {
                warn!("[dem-terrain] worker bake {stage:?} failed: {e}");
                if matches!(stage, lunco_terrain_bake::BakeStage::Full) {
                    commands.entity(entity).remove::<DemWorkerJob>();
                } else {
                    commands.entity(entity).remove::<(DemTerrainRequest, DemWorkerJob)>();
                }
            }
        }
    }
    if any_full {
        // Cached tile meshes are stale now → drop so re-baked tiles pick up the new
        // heights; despawn old scatter (rocks/overlays) so they re-scatter.
        commands.insert_resource(crate::stream_viz::LodMeshCache::default());
        for e in &scattered {
            commands.entity(e).try_despawn();
        }
    }
}

/// Swap a freshly (re)baked surface oracle into a live terrain: replace
/// `DemHeightField`, rebuild the static mesh if it uses one, arm the debounced
/// static-collider rebuild (unless a collider ring streams physics), and invalidate
/// the LOD tiles so they refresh near-camera-first with no despawn flash. The web
/// worker's full-grid reply composes the oracle then calls this.
#[cfg(target_arch = "wasm32")]
#[allow(clippy::too_many_arguments)]
fn swap_terrain_grid(
    commands: &mut Commands,
    entity: Entity,
    oracle: std::sync::Arc<crate::oracle::SurfaceOracle>,
    collider_ring: bool,
    hf: &mut crate::stream_viz::DemHeightField,
    tiles: Option<Mut<crate::stream_viz::LodTiles>>,
    pending: Option<Mut<crate::stream_viz::PendingTileBakes>>,
    has_static_mesh: bool,
    meshes: Option<&mut Assets<Mesh>>,
) {
    // Defer the (heavy) static-collider rebuild so the VISUAL swap lands immediately
    // and physics reconverges shortly after. Collider-ring terrains stream physics.
    if !collider_ring {
        commands
            .entity(entity)
            .insert(DemColliderDirty(Timer::from_seconds(COLLIDER_DEBOUNCE_SECS, TimerMode::Once)));
    }
    if has_static_mesh {
        if let Some(meshes) = meshes {
            let MeshData { positions, normals, uvs, indices } = oracle.materialize().to_mesh_data();
            let handle = meshes.add(lunco_obstacle_field::grid_mesh(
                positions,
                normals,
                uvs,
                indices,
                bevy::asset::RenderAssetUsages::default(),
            ));
            commands.entity(entity).insert(Mesh3d(handle));
        }
    }
    *hf = crate::stream_viz::DemHeightField(oracle);
    // Progressive refresh: reap any already-stale tiles (keep ≤1 generation of
    // cover), bump the generation so live tiles re-bake near-first, drop in-flight
    // bakes from the OLD heights.
    if let Some(mut tiles) = tiles {
        for e in tiles.reap_stale() {
            commands.entity(e).try_despawn();
        }
        tiles.invalidate();
    }
    if let Some(mut pending) = pending {
        *pending = crate::stream_viz::PendingTileBakes::default();
    }
    // Scatter layers re-run once the applied-marker is gone (next frame).
    commands.entity(entity).remove::<crate::terrain_layers::TerrainLayersApplied>();
}

/// In-flight off-thread **visual** re-stamp for a terrain: clones the pristine base
/// grid and stamps the layers into it, producing the new working [`HeightGrid`]. This
/// is the FAST path (clone + stamp only); the expensive static-collider rebuild is
/// split off into a separate debounced task ([`DemColliderDirty`]/[`DemColliderTask`])
/// so the visible terrain updates ~immediately and physics catches up after.
#[derive(Component)]
pub(crate) struct DemRestampTask(Task<std::sync::Arc<crate::oracle::SurfaceOracle>>);

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
pub(crate) struct DemRestampPending;

/// Debounce armed on each layer-stack change; the re-stamp only kicks off once it
/// elapses with no further change. Re-stamping the *whole* DEM is heavy (clone +
/// tens of thousands of crater stamps + collider build), so coalescing a slider
/// drag's many changes into ONE trailing re-stamp is what keeps live tuning from
/// piling up back-to-back full re-bakes (the "it stuck" when changing repeatedly).
#[derive(Component)]
struct DemRestampDebounce(Timer);

/// Settle delay before a layer edit triggers the off-thread re-stamp. Long enough to
/// swallow a continuous slider drag, short enough to feel responsive on release.
/// (Halved from 0.3 once the all-analytic restamp stopped cloning the base grid —
/// the re-compose itself is now microseconds, so the debounce IS the edit latency.)
const RESTAMP_DEBOUNCE_SECS: f32 = 0.15;

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
    curvature_radius: Option<f64>,
) {
    let base_grid = base.0.clone();
    let base_key = base.1;
    let layers: Vec<_> = stack.0.iter().map(|e| e.layer.clone()).collect();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        // Off-thread body → own Tracy zone (per-system spans don't reach here).
        let _span = bevy::log::info_span!("terrain_restamp").entered();
        let half_extent = base_grid.half_extent;
        // All-analytic stacks (the norm: craters/edits/over-zoom are oracle
        // modifiers) reuse the base grid Arc directly — cloning + re-stamping
        // the whole raster was a multi-MB memcpy on EVERY brush stroke, for a
        // grid that came out identical.
        let mut contributions: Vec<_> =
            layers.iter().filter_map(|l| l.height_modifier(half_extent)).collect();
        if let Some(r) = curvature_radius {
            contributions.push(crate::oracle::curvature_contribution(r, half_extent));
        }
        let oracle = if layers.iter().any(|l| l.stamps()) {
            // A stamp mutates the raster → a genuinely new grid, so its key must be
            // re-folded (no reuse possible).
            let mut grid = (*base_grid).clone();
            for layer in &layers {
                layer.stamp(&mut grid);
            }
            crate::oracle::SurfaceOracle::new(std::sync::Arc::new(grid), contributions)
        } else {
            // Same base Arc → reuse the cached key instead of re-hashing ~10M
            // heights off-thread on every brush stroke.
            crate::oracle::SurfaceOracle::new_with_base_key(base_grid, contributions, base_key)
        };
        std::sync::Arc::new(oracle)
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
    curvature: Option<Res<crate::oracle::TerrainBodyCurvature>>,
) {
    let curvature_radius = curvature.map(|c| c.radius_m);
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
        spawn_restamp_task(&mut commands, entity, base, &stack, curvature_radius);
    }
}

/// Collect finished off-thread re-stamps: swap in the new heights + collider, then
/// trigger a **progressive** visual refresh — bump the streaming generation so live
/// tiles go stale and re-bake near-camera-first (covering the surface meanwhile)
/// rather than all being despawned at once. Rocks/overlays re-scatter next frame.
#[allow(clippy::type_complexity)]
pub(crate) fn finish_dem_restamp(
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
        Has<TerrainRescatter>,
    )>,
    scattered: Query<Entity, With<crate::terrain_layers::TerrainScatterEntity>>,
    mut meshes: Option<ResMut<Assets<Mesh>>>,
    mut mesh_cache: ResMut<crate::stream_viz::LodMeshCache>,
) {
    use bevy::tasks::futures_lite::future;
    // Whether ANY terrain did a WHOLE-terrain re-bake this pass (spec change / load) —
    // only then do the (global) scatter entities need dropping + rebuilding.
    let mut any_full = false;
    for (
        entity,
        mut task,
        src,
        mut hf,
        tiles,
        pending,
        has_static_mesh,
        was_pending,
        dirty,
        rescatter,
    ) in &mut tasks
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
                    lunco_obstacle_field::grid_mesh(
                        positions,
                        normals,
                        uvs,
                        indices,
                        bevy::asset::RenderAssetUsages::default(),
                    ),
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
        // Tell the derived-maps re-bake whether this swap was a bounded edit: if
        // so it keeps the published maps live (correct outside the footprint)
        // instead of popping the far field to the procedural fallback.
        commands
            .entity(entity)
            .try_insert(crate::derived_layers::DerivedDirtyRegion { bounded: scoped });
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
        mesh_cache.drop_region(entity, dirty_bounds, half);

        // A bounded edit leaves the crater/rock fields untouched, so DON'T re-scatter
        // (that despawn+respawn of every rock is a big part of the per-edit cost). A
        // whole-terrain change re-scatters: clear the applied-marker so scatter re-runs.
        // The scatter-only tier (`TerrainRescatter` — e.g. a placed rock) re-scatters
        // too, while keeping the tile re-bake scoped to the marked bounds.
        if !scoped || rescatter {
            commands.entity(entity).try_remove::<crate::terrain_layers::TerrainLayersApplied>();
            commands.entity(entity).try_remove::<TerrainRescatter>();
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
            // Off-thread body → own Tracy zone.
            let _span = bevy::log::info_span!("terrain_static_collider_bake").entered();
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
            .try_insert((RigidBody::Static, collider));
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

/// Scatter-only dirty tier: the stack gained/lost a SCATTER layer (e.g. a placed
/// rock) but heights are untouched — the re-bake stays scoped (no tile/mesh-cache
/// invalidation beyond the marked bounds) yet the scatter layers still re-run.
/// Consumed by [`finish_dem_restamp`] alongside [`TerrainDirty`].
#[derive(Component)]
pub struct TerrainRescatter;

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

/// Mark a placed crater's footprint dirty (see [`TerrainDirty`]) — the ejecta
/// apron reaches past the rim radius, so the footprint uses the crater reach.
fn on_place_crater_dirty(
    trigger: On<PlaceCrater>,
    mut commands: Commands,
    mut q: Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
) {
    let ev = trigger.event();
    if ev.radius <= 0.0 {
        return;
    }
    let reach = ev.radius * lunco_terrain_core::CRATER_REACH as f32;
    accumulate_terrain_dirty(&mut commands, &mut q, Some(edit_command_aabb(ev.x, ev.z, reach)));
}

/// A placed rock changes no heights: mark its tiny footprint dirty (keeps the
/// tile re-bake scoped ≈ no-op) and arm the scatter-only tier so the rock
/// actually spawns (see [`TerrainRescatter`]).
fn on_place_rock_dirty(
    trigger: On<PlaceRock>,
    mut commands: Commands,
    mut q: Query<(Entity, Option<&mut TerrainDirty>), With<DemBaseGrid>>,
) {
    let ev = trigger.event();
    let size = ev.size_or_default();
    accumulate_terrain_dirty(&mut commands, &mut q, Some(edit_command_aabb(ev.x, ev.z, size)));
    for (e, _) in q.iter() {
        commands.entity(e).try_insert(TerrainRescatter);
    }
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

/// Force a whole-oracle re-compose when parent-body curvature arrives or
/// changes. Two orderings need covering: the [`crate::oracle::TerrainBodyCurvature`]
/// resource landing AFTER a DEM build captured `None` (resource change → regen),
/// and a build finishing AFTER the resource landed but having been SPAWNED
/// before it (fresh `DemHeightField` while the resource exists → regen). When
/// the finished build already composed the same curvature, the re-stamp
/// produces an identical `surface_key`, so tile bakes hit their caches — the
/// redundant pass is cheap.
fn restamp_on_curvature(
    curvature: Option<Res<crate::oracle::TerrainBodyCurvature>>,
    new_dems: Query<(), Added<crate::stream_viz::DemHeightField>>,
    mut regen: MessageWriter<RegenerateTerrainLayers>,
    mut had: Local<bool>,
) {
    let Some(c) = curvature else {
        // Resource removed (site anchor gone) → flatten surviving DEMs back.
        if *had {
            *had = false;
            regen.write(RegenerateTerrainLayers);
        }
        return;
    };
    *had = true;
    if c.is_changed() || !new_dems.is_empty() {
        regen.write(RegenerateTerrainLayers);
    }
}

/// Register the DEM-terrain command + spawn systems. Called from
/// [`crate::plugin::TerrainSurfacePlugin`].
pub(crate) fn register(app: &mut App) {
    app.register_type::<SpawnDemTerrain>()
        .init_resource::<TerrainEditSeq>()
        .init_resource::<TerrainGenStatus>()
        .init_resource::<crate::stream_viz::LodMeshCache>()
        .add_message::<RegenerateTerrainLayers>()
        .add_observer(on_obstacle_spec_rebuild_layers)
        // Dirty-region markers — registered BEFORE the command observers (below) so a
        // remove reads the edit's bounds before the removal applies.
        .add_observer(on_brush_terrain_dirty)
        .add_observer(on_flatten_terrain_dirty)
        .add_observer(on_place_crater_dirty)
        .add_observer(on_place_rock_dirty)
        .add_observer(on_remove_terrain_layer_dirty)
        .add_observer(on_obstacle_spec_dirty)
        .add_systems(
            Update,
            (
                start_dem_builds,
                finish_dem_builds,
                restamp_on_curvature.before(start_dem_restamp),
                start_dem_restamp,
                finish_dem_restamp,
                start_dem_collider,
                finish_dem_collider,
                update_terrain_gen_status,
            ),
        );
    // WEB: register the off-thread DEM bake worker URL (staged by build_web.sh next
    // to the wasm) and the reply-draining system that applies the coarse-then-full
    // grids. The worker is spawned lazily on the first bake dispatch.
    #[cfg(target_arch = "wasm32")]
    {
        lunco_terrain_bake::worker_client::set_worker_url("./dem-worker/dem_worker_bootstrap.js");
        app.add_observer(cancel_worker_job_on_remove);
        app.add_systems(Update, finish_dem_worker);
    }
    register_all_commands(app);
}
