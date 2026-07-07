//! Pure DEM bake pipeline — shared verbatim by the native async task
//! (`lunco-terrain-surface::terrain::start_dem_builds`) and the wasm Web Worker
//! (`dem_worker`).
//!
//! The heavy load-time cost of a DEM terrain is the GeoTIFF decode (~40 MB) plus
//! the crater stamp (thousands of additive bowls) plus the resample/upscale
//! passes. On native that runs on an `AsyncComputeTaskPool` thread; on wasm the
//! same pool degrades to the page's main thread, which froze the tab for 15-30 s.
//! This crate factors that compute into ONE bevy/avian-free function
//! ([`bake_grid`] / [`finish_bake`]) so a real Web Worker can run it off the main
//! thread while native keeps its threaded path — the SAME code both ways.
//!
//! The output is a [`HeightGrid`]; the (cheap) avian collider + Bevy mesh derive
//! stays with the caller in `lunco-terrain-surface`, which owns those types.

pub mod bake;
pub mod dem;
pub mod stamp;

#[cfg(target_arch = "wasm32")]
pub mod worker_client;

use lunco_obstacle_field::field::HeightGrid;
use lunco_obstacle_field::spec::CraterLayer;
use serde::{Deserialize, Serialize};

/// A serializable stamp operation. The worker reconstructs these from the
/// terrain's layer stack (via `TerrainLayer::stamp_spec`) and applies the SAME
/// stamp code the native path runs — deterministic from the seed, so worker and
/// main agree with nothing but the spec transferred.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StampSpec {
    Craters { layer: CraterLayer, seed: u64 },
}

impl StampSpec {
    /// Apply this stamp into the working grid. Returns the feature count stamped.
    pub fn apply(&self, grid: &mut HeightGrid) -> usize {
        match self {
            StampSpec::Craters { layer, seed } => stamp::stamp_spec_craters(grid, layer, *seed),
        }
    }
}

/// The immutable bake parameters (from `DemTerrainRequest`) plus the serializable
/// stamp specs. Built inline on native; bincode-serialized to the worker on web.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DemBakeJob {
    /// Half side length (m) of the centred region realized at native resolution.
    /// `f64::INFINITY` = the whole DEM.
    pub half_window: f64,
    /// Visual-quality downsample target (samples/side); `0` = native.
    pub target_res: usize,
    /// Intelligent-upscaling factor applied to the ground before stamping.
    pub detail_upsample: usize,
    /// Geometry stamp layers (craters, …) applied into the working grid.
    pub stamps: Vec<StampSpec>,
}

/// Coarse-first progressive staging: the worker emits a low-res preview grid
/// first (fast — terrain + collider appear, rovers settle), then the full-res
/// grid, which the main thread swaps in via the live re-stamp machinery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BakeStage {
    Coarse,
    Full,
}

/// Cap on the coarse preview's samples-per-side — high enough to read the relief
/// and settle physics, low enough to bake in ~a second after the shared decode.
pub const COARSE_RES: usize = 384;

/// One bake output (either stage). `grid` is crater-stamped; `base_grid` is the
/// pre-crater working grid retained for a live re-bake (native), a clone of the
/// stamped grid on web (doc-backed terrains never live-regenerate).
pub struct BakedGrid {
    pub grid: HeightGrid,
    pub base_grid: HeightGrid,
    pub site: String,
    /// Native crop resolution before any resample (for honest logging).
    pub native_res: usize,
    /// Tile resolution actually produced (= native crop, or the resample target).
    pub res: usize,
    pub stage: BakeStage,
}

/// Decode the DEM once into its full native grid — the expensive GeoTIFF decode.
/// Kept separate from [`finish_bake`] so a coarse + full pass share ONE decode.
pub fn decode_raw(meta_yaml: &str, tif: &[u8]) -> Result<(dem::DemMetadata, HeightGrid), String> {
    let meta = dem::DemMetadata::from_yaml_str(meta_yaml).map_err(|e| e.to_string())?;
    let grid = dem::height_grid_from_geotiff(tif, &meta).map_err(|e| e.to_string())?;
    Ok((meta, grid))
}

/// Produce ONE stage's [`BakedGrid`] from the pre-decoded native grid. The Coarse
/// stage forces [`COARSE_RES`] and skips the detail upscale so it finishes fast;
/// the Full stage honours the job's `target_res` + `detail_upsample`.
///
/// This is the crop → resample → upscale → stamp core that `start_dem_builds`
/// used to run inline — identical, so native behaviour is unchanged.
pub fn finish_bake(raw: &HeightGrid, site: &str, job: &DemBakeJob, stage: BakeStage) -> BakedGrid {
    // Crop the playable region at native resolution (mesh + collider share it).
    let tile = bake::crop_centered(raw, job.half_window);
    let native_res = tile.res;
    // Stage/quality downsample. Coarse forces COARSE_RES; Full honours target_res.
    let target = match stage {
        BakeStage::Coarse => COARSE_RES.min(native_res.saturating_sub(1)),
        BakeStage::Full => job.target_res,
    };
    let mut tile = if target > 0 && target < native_res {
        bake::resample(&tile, tile.half_extent as f64, target)
    } else {
        tile
    };
    // Intelligent upscaling (Full only — the coarse preview stays coarse): bilinearly
    // upscale the coarse ground to a finer working grid BEFORE the crater stamp, so
    // rims resolve below the DEM sampling. Decouples crater fidelity from DEM res.
    if stage == BakeStage::Full && job.detail_upsample > 1 {
        let up = (tile.res - 1) * job.detail_upsample + 1;
        tile = bake::resample(&tile, tile.half_extent as f64, up);
    }
    let res = tile.res;
    // Retain the crater-FREE grid so a live regenerate re-stamps off it (native).
    let base_grid = tile.clone();
    // Apply the geometry STAMP layers (craters, …) into the working grid so both
    // the streamed tiles and the heightfield collider carry the same features.
    for stamp in &job.stamps {
        stamp.apply(&mut tile);
    }
    BakedGrid { grid: tile, base_grid, site: site.to_string(), native_res, res, stage }
}

/// Full single-pass bake (native path): decode + the Full stage in one call.
pub fn bake_grid(meta_yaml: &str, tif: &[u8], job: &DemBakeJob) -> Result<BakedGrid, String> {
    let (meta, raw) = decode_raw(meta_yaml, tif)?;
    Ok(finish_bake(&raw, &meta.site_id, job, BakeStage::Full))
}

// ── Web-Worker wire protocol ──────────────────────────────────────────────────
// The bulk (heights) rides a Transferable `ArrayBuffer` (zero-copy); this small
// bincode header carries the scalars alongside it. One header per emitted stage.

/// Worker → main per-stage result header (bincode). The `res²` f64 heights ride a
/// separate transferred `ArrayBuffer`; `res` + `half_extent` rebuild the grid.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BakeReplyHeader {
    /// Correlates with the dispatched job (an entity id, low bits).
    pub id: u32,
    pub stage: BakeStage,
    /// `Some` on failure (decode/parse) — no heights buffer accompanies it.
    pub err: Option<String>,
    pub site: String,
    pub res: usize,
    pub half_extent: f32,
    pub native_res: usize,
}
