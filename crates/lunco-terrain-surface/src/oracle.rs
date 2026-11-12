//! The composed **surface oracle** — the single analytic height truth every
//! consumer samples.
//!
//! A DEM terrain's surface is `modifiers ∘ base`: a raster base grid (the cropped
//! DEM, plus any layer that genuinely rasterises) with an ordered stack of
//! **analytic** [`HeightModifier`]s folded over it — craters, runtime edits, and
//! (later) procedural over-zoom detail. The composed [`SurfaceOracle`] is what the
//! CDLOD tile baker, the collider ring, the derived-layer texture bakes, the rock
//! scatter, and the `TerrainHeight` query all sample — **one function, every
//! consumer**, so visuals and physics converge by construction and feature
//! crispness is bounded only by how finely a consumer samples, never by a grid
//! resolution (see `docs/architecture/terrain-substrate.md`).
//!
//! Purity is load-bearing: the oracle is a deterministic function of the base
//! grid + modifier parameters, so derived artifacts (tiles, colliders, texture
//! maps) stay content-addressable and peer-identical. [`SurfaceOracle::content_key`]
//! folds every modifier's parameter hash for exactly that — cache keys downstream
//! mix it with their own inputs.

use std::sync::{Arc, OnceLock};

use bevy::prelude::Component;
use lunco_obstacle_field::field::HeightGrid;
use lunco_terrain_core::{HeightEnclosure, HeightModifier, HeightPlane, HeightSource};

/// The composed surface oracle retained on a terrain entity.
///
/// This is the domain product shared by visual and physics consumers: it is not
/// owned by visual streaming, and it carries no render state. `Arc` lets native
/// and worker bakes sample the same immutable oracle without copying it.
#[derive(Component)]
pub struct DemHeightField(pub Arc<SurfaceOracle>);

/// One layer's analytic contribution to the composed surface: the modifier to
/// fold, plus a content hash of the parameters that produced it (folds into
/// downstream cache keys, since the modifier itself is opaque).
#[derive(Clone)]
pub struct HeightContribution {
    pub modifier: Arc<dyn HeightModifier>,
    /// Deterministic hash of the layer's generating parameters (spec + seed).
    pub content_key: u64,
}

/// Parent-body curvature for site-anchored DEM terrains. Inserted by the
/// celestial layer when the scene anchors to a body's surface; while present,
/// every oracle composition folds a [`lunco_terrain_core::BodyCurvature`]
/// modifier LAST, so the tangent-plane DEM curves down onto the body sphere by
/// the physical sagitta (≈ 37 m at an 8 km patch edge on the Moon). Because it
/// enters through the normal contribution path, tile meshes, colliders, the
/// shadow heightfield, and height queries all agree — and `content_key` folds
/// it, so downstream caches invalidate correctly.
#[derive(bevy::prelude::Resource, Clone, Copy, Debug, PartialEq)]
pub struct TerrainBodyCurvature {
    /// Body mean radius (metres).
    pub radius_m: f64,
}

/// The [`TerrainBodyCurvature`] fold as a [`HeightContribution`]. Composition
/// sites append this AFTER the layer stack's contributions so every authored
/// relief modifier receives the same physical curvature transform.
///
/// `datum_m` is the site's ground elevation, used only to compute its local
/// body radius. The cropped DEM values are not altered or faded.
pub fn curvature_contribution(radius_m: f64, datum_m: f64) -> HeightContribution {
    let m = lunco_terrain_core::BodyCurvature::new(radius_m, datum_m);
    let mut key = lunco_precompute::Fnv1a::new();
    key.write_u64(0xB0D1_C42E); // domain tag: body curvature
    key.write_u64(m.radius_m.to_bits());
    key.write_u64(m.datum_m.to_bits());
    HeightContribution {
        modifier: Arc::new(m),
        content_key: key.finish(),
    }
}

/// The composed surface: raster `base` + ordered analytic `modifiers`. Cheap to
/// clone-share (`Arc` it once per terrain); `Send + Sync` so off-thread bakes
/// sample it directly.
#[derive(Clone)]
pub struct SurfaceOracle {
    /// The raster base every modifier folds over (cropped DEM working grid).
    base: Arc<HeightGrid>,
    /// Optional registered regional continuation over the unchanged base grid.
    regional: Option<Arc<lunco_terrain_bake::regional::RegisteredRegionalSurface>>,
    /// Analytic modifiers in fold order (USD prim order — index 0 first).
    modifiers: Vec<Arc<dyn HeightModifier>>,
    /// Folded [`HeightContribution::content_key`]s (0 with no modifiers).
    content_key: u64,
    /// Content hash of the raster base (heights + geometry), computed once at
    /// compose time so per-tile cache keys never re-fold the multi-million-point
    /// grid.
    base_key: u64,
    /// Identity of the local grid, independent of any regional continuation.
    local_base_key: u64,
    /// Geometry-wide certificate prepared once by asynchronous presentation work.
    /// Clones share immutable geometry; filtered variants own a fresh certificate.
    global_bounds: Arc<OnceLock<Result<[f64; 2], String>>>,
}

fn checked_height_bounds(bounds: [f64; 2]) -> Result<[f64; 2], String> {
    if bounds[0].is_finite() && bounds[1].is_finite() && bounds[0] <= bounds[1] {
        Ok(bounds)
    } else {
        Err("invalid terrain height enclosure".into())
    }
}

/// Check the complete raw grid before an enclosure reader indexes its posts.
/// The checked product also bounds the preparation scan and regional budget.
fn validated_grid_cell_count(grid: &HeightGrid) -> Result<usize, String> {
    if grid.res < 2
        || grid.res.checked_mul(grid.res) != Some(grid.heights.len())
        || !grid.half_extent.is_finite()
        || grid.half_extent <= 0.0
        || !grid.spacing().is_finite()
        || grid.spacing() <= 0.0
        || grid.heights.iter().any(|height| !height.is_finite())
    {
        return Err("invalid terrain raster for global height enclosure".into());
    }
    (grid.res - 1)
        .checked_mul(grid.res - 1)
        .ok_or_else(|| "global terrain height enclosure cell count overflow".into())
}

impl SurfaceOracle {
    pub(crate) fn has_regional_surface(&self) -> bool {
        self.regional.is_some()
    }
    /// Ordered modifier composition shared by scalar and affine query products.
    fn fold_modifiers<T>(
        &self,
        mut input: T,
        mut apply: impl FnMut(&dyn HeightModifier, T) -> Result<T, String>,
    ) -> Result<T, String> {
        for modifier in &self.modifiers {
            input = apply(modifier.as_ref(), input)?;
        }
        Ok(input)
    }

    /// Conservative scalar enclosure; scalar consumers do not request analytic
    /// gradients or lose the tight independent height interval.
    pub(crate) fn height_bounds_in_cell(
        &self,
        cell: [usize; 2],
        rect: [f64; 4],
        remaining_cells: &mut usize,
    ) -> Result<[f64; 2], String> {
        let initial = checked_height_bounds(match &self.regional {
            Some(regional) => regional
                .height_bounds(rect, remaining_cells)
                .map_err(str::to_owned)?,
            None => self.base.cell_height_bounds(cell, rect),
        })?;
        self.fold_modifiers(initial, |modifier, input| {
            checked_height_bounds(
                modifier
                    .height_bounds(rect, input)
                    .ok_or("terrain modifier has no conservative height bounds")?,
            )
        })
    }

    /// Conservative enclosure of the entire admitted composed surface, including
    /// raster interpolation, regional continuation and every ordered modifier.
    /// The first call scans the bounded source rasters. Invoke it in asynchronous
    /// preparation, never from a render, navigation or physics hot path; identical
    /// immutable clones subsequently reuse the result, including a failed proof.
    pub fn global_height_bounds(&self) -> Result<[f64; 2], String> {
        self.global_bounds
            .get_or_init(|| self.compute_global_height_bounds())
            .clone()
    }

    /// Read an already prepared optional clipping certificate without starting
    /// a raster scan. Synchronous presentation consumers keep uncertain nodes
    /// eligible when no certificate has been prepared by a worker.
    pub fn cached_global_height_bounds(&self) -> Option<Result<[f64; 2], String>> {
        self.global_bounds.get().cloned()
    }

    fn compute_global_height_bounds(&self) -> Result<[f64; 2], String> {
        let local_cells = validated_grid_cell_count(&self.base)?;
        let half = f64::from(self.domain_half_extent());
        let rect = [-half, -half, half, half];
        let base = match &self.regional {
            Some(regional) => {
                let regional_cells = validated_grid_cell_count(regional.regional_grid())?;
                // The registered owner examines local, regional and regional-edge
                // grids at most once each for this full-domain rectangle.
                let mut remaining = regional_cells
                    .checked_mul(2)
                    .and_then(|regional| local_cells.checked_add(regional))
                    .ok_or("global regional height enclosure cell count overflow")?;
                regional
                    .height_bounds(rect, &mut remaining)
                    .map_err(str::to_owned)?
            }
            None => {
                let spacing = f64::from(self.base.spacing());
                let last = self.base.res - 2;
                let cut = |index: usize| (-half + index as f64 * spacing).clamp(-half, half);
                let mut bounds = [f64::INFINITY, f64::NEG_INFINITY];
                for z in 0..=last {
                    for x in 0..=last {
                        let cell_rect = [
                            cut(x),
                            cut(z),
                            if x == last { half } else { cut(x + 1) },
                            if z == last { half } else { cut(z + 1) },
                        ];
                        let cell =
                            checked_height_bounds(self.base.cell_height_bounds([x, z], cell_rect))?;
                        bounds[0] = bounds[0].min(cell[0]);
                        bounds[1] = bounds[1].max(cell[1]);
                    }
                }
                bounds
            }
        };
        self.fold_modifiers(checked_height_bounds(base)?, |modifier, input| {
            checked_height_bounds(
                modifier
                    .height_bounds(rect, input)
                    .ok_or("terrain modifier has no conservative height bounds")?,
            )
        })
    }

    /// The raster tangent is a fit only. Its restricted Bézier residual and
    /// every subsequent modifier certificate prove the final composed model.
    fn base_height_plane(&self, rect: [f64; 4]) -> Result<HeightPlane, String> {
        let origin = [
            rect[0] + (rect[2] - rect[0]) * 0.5,
            rect[1] + (rect[3] - rect[1]) * 0.5,
        ];
        let grid = self
            .regional
            .as_ref()
            .map_or(self.base.as_ref(), |regional| {
                let half = f64::from(regional.local_grid().half_extent);
                if origin[0].abs() <= half && origin[1].abs() <= half {
                    regional.local_grid().as_ref()
                } else {
                    regional.regional_grid().as_ref()
                }
            });
        let (_, gradient, _) = grid.height_and_derivatives_at(origin[0], origin[1]);
        let height = match &self.regional {
            Some(regional) => regional
                .height_at(origin[0], origin[1])
                .map_err(str::to_owned)?,
            None => self.base.height_at(origin[0], origin[1]),
        };
        let plane = HeightPlane {
            origin,
            height,
            gradient,
        };
        if plane.is_finite() {
            Ok(plane)
        } else {
            Err("invalid terrain affine reference".into())
        }
    }

    /// One scalar-plus-affine model for the full immutable composed source.
    /// Analytic additive modifiers may update the slope as well as the residual.
    pub(crate) fn height_enclosure_in_cell(
        &self,
        cell: [usize; 2],
        rect: [f64; 4],
        remaining_cells: &mut usize,
    ) -> Result<HeightEnclosure, String> {
        if rect.into_iter().any(|v| !v.is_finite()) || rect[0] > rect[2] || rect[1] > rect[3] {
            return Err("invalid terrain affine enclosure input".into());
        }
        let plane = self.base_height_plane(rect)?;
        let checked = |model: HeightEnclosure| {
            if model.is_valid(rect) {
                Ok(model)
            } else {
                Err("invalid composed terrain affine enclosure".to_owned())
            }
        };
        let initial = checked(match &self.regional {
            Some(regional) => regional
                .height_enclosure(rect, plane, remaining_cells)
                .map_err(str::to_owned)?,
            None => self.base.cell_height_enclosure(cell, rect, plane),
        })?;
        self.fold_modifiers(initial, |modifier, input| {
            checked(
                modifier
                    .height_enclosure(rect, input)
                    .ok_or("terrain modifier has no conservative affine enclosure")?,
            )
        })
    }
    /// Number of postings across the complete sampled terrain domain.
    pub fn domain_resolution(&self) -> usize {
        self.regional
            .as_ref()
            .map_or(self.base.res, |regional| regional.regional_grid().res)
    }
    /// Spacing used for conservative queries over the complete terrain domain.
    pub fn occlusion_spacing(&self) -> f64 {
        self.regional
            .as_ref()
            .map_or(f64::from(self.base.spacing()), |regional| {
                f64::from(regional.regional_grid().spacing())
            })
    }
    /// An oracle that is just the raster base — no analytic layers.
    pub fn bare(base: Arc<HeightGrid>) -> Self {
        let base_key = lunco_terrain_bake::grid_content_key(&base);
        Self {
            base,
            regional: None,
            modifiers: Vec::new(),
            content_key: 0,
            base_key,
            local_base_key: base_key,
            global_bounds: Arc::new(OnceLock::new()),
        }
    }

    /// Compose `base` with the layers' analytic contributions, in order. Folds the
    /// base grid to derive its content key — use [`new_with_base_key`] on a live
    /// re-compose that reuses the same base Arc to skip that fold.
    ///
    /// [`new_with_base_key`]: SurfaceOracle::new_with_base_key
    pub fn new(base: Arc<HeightGrid>, contributions: Vec<HeightContribution>) -> Self {
        let base_key = lunco_terrain_bake::grid_content_key(&base);
        Self::new_with_base_key(base, contributions, base_key)
    }

    /// Like [`new`], but takes the base grid's precomputed content key instead of
    /// re-folding it. The caller MUST pass
    /// [`lunco_terrain_bake::grid_content_key`] for `base` — a live
    /// re-stamp caches this per retained base grid so brush strokes never re-hash
    /// the multi-million-point raster.
    ///
    /// [`new`]: SurfaceOracle::new
    pub fn new_with_base_key(
        base: Arc<HeightGrid>,
        contributions: Vec<HeightContribution>,
        base_key: u64,
    ) -> Self {
        let mut key = lunco_precompute::Fnv1a::new();
        let mut modifiers = Vec::with_capacity(contributions.len());
        for c in &contributions {
            key.write_u64(c.content_key);
        }
        let content_key = if contributions.is_empty() {
            0
        } else {
            key.finish()
        };
        for c in contributions {
            modifiers.push(c.modifier);
        }
        Self {
            base,
            regional: None,
            modifiers,
            content_key,
            base_key,
            local_base_key: base_key,
            global_bounds: Arc::new(OnceLock::new()),
        }
    }

    /// Compose modifiers over an admitted registered local/regional surface.
    pub fn registered(
        regional: Arc<lunco_terrain_bake::regional::RegisteredRegionalSurface>,
        contributions: Vec<HeightContribution>,
    ) -> Self {
        let mut surface = Self::new_with_base_key(
            regional.local_grid().clone(),
            contributions,
            regional.content_key(),
        );
        surface.local_base_key = regional.local_content_key();
        surface.regional = Some(regional);
        surface
    }

    /// The raster base grid (extents, spacing, raw ground reads).
    pub fn grid(&self) -> &Arc<HeightGrid> {
        &self.base
    }

    /// Half side length (metres) of the terrain footprint.
    pub fn half_extent(&self) -> f32 {
        self.base.half_extent
    }

    /// Half extent of the complete admitted surface, including regional data.
    pub fn domain_half_extent(&self) -> f32 {
        self.regional
            .as_ref()
            .map_or(self.base.half_extent, |regional| {
                regional.regional_grid().half_extent
            })
    }

    /// Rendering-boundary width for fading maps outside the local crop.
    pub fn regional_map_blend_m(&self) -> f32 {
        self.regional
            .as_ref()
            .map_or(0.0, |regional| regional.blend_width_m() as f32)
    }

    /// Convert local-crop UV offsets to regional-crop UV offsets at the GPU boundary.
    pub fn regional_uv_scale(&self) -> f32 {
        self.regional.as_ref().map_or(0.0, |regional| {
            (f64::from(self.base.half_extent) / f64::from(regional.regional_grid().half_extent))
                as f32
        })
    }

    /// Metres between adjacent base-grid samples (the raster resolution floor —
    /// analytic modifiers resolve *below* this).
    pub fn spacing(&self) -> f32 {
        self.base.spacing()
    }

    /// Folded content hash of every modifier's parameters — mix into any cache
    /// key derived from this surface (the base grid hashes separately).
    pub fn content_key(&self) -> u64 {
        self.content_key
    }

    /// Content identity of the COMPLETE composed surface (base heights +
    /// modifier parameters). Two oracles with equal `surface_key` produce
    /// byte-identical samples everywhere — the per-tile bake cache keys on this
    /// so it never re-hashes the full grid.
    pub fn surface_key(&self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(self.base_key);
        h.write_u64(self.content_key);
        h.finish()
    }

    /// Identity of samples restricted to the unchanged local footprint.
    pub fn local_surface_key(&self) -> u64 {
        let mut h = lunco_precompute::Fnv1a::new();
        h.write_u64(self.local_base_key);
        h.write_u64(self.content_key);
        h.finish()
    }

    /// Whether any analytic modifier is stacked over the base.
    pub fn has_modifiers(&self) -> bool {
        !self.modifiers.is_empty()
    }

    /// A variant of this oracle **Nyquist-gated** for a consumer sampling at
    /// `min_wavelength` metres: band-limitable modifiers (procedural over-zoom,
    /// craters) swap in a gated copy so sub-sample features widen or fade out
    /// instead of aliasing — and synthesis cost drops with them. Modifiers with
    /// no gated form (brushes, flattens) pass through untouched. Cheap (clones a
    /// Vec of `Arc`s); call it per bake with that bake's sample spacing.
    pub fn detail_limited(&self, min_wavelength: f64) -> SurfaceOracle {
        let modifiers = self
            .modifiers
            .iter()
            .map(|m| {
                m.with_min_wavelength(min_wavelength)
                    .unwrap_or_else(|| m.clone())
            })
            .collect();
        SurfaceOracle {
            base: self.base.clone(),
            regional: self.regional.clone(),
            modifiers,
            content_key: self.content_key,
            base_key: self.base_key,
            local_base_key: self.local_base_key,
            global_bounds: Arc::new(OnceLock::new()),
        }
    }

    /// A variant that is BOTH Nyquist-gated for `min_wavelength` and scoped to
    /// the world box `[min, max]` — the form every region-shaped bake (tile
    /// mesh, collider tile, derived maps) should sample.
    ///
    /// Placement-backed modifiers (crater fields) gather their footprint once
    /// here instead of resolving "which placements are near?" per sample;
    /// modifiers with no region form fall back to the plain gate, and those with
    /// neither pass through untouched. The result is value-identical to
    /// [`detail_limited`] **inside the box only** — never hold it past the bake
    /// that scoped it.
    ///
    /// [`detail_limited`]: SurfaceOracle::detail_limited
    pub fn detail_limited_region(
        &self,
        min_wavelength: f64,
        min: [f64; 2],
        max: [f64; 2],
    ) -> SurfaceOracle {
        let modifiers = self
            .modifiers
            .iter()
            .map(|m| {
                m.for_region(min, max, min_wavelength)
                    .or_else(|| m.with_min_wavelength(min_wavelength))
                    .unwrap_or_else(|| m.clone())
            })
            .collect();
        SurfaceOracle {
            base: self.base.clone(),
            regional: self.regional.clone(),
            modifiers,
            content_key: self.content_key,
            base_key: self.base_key,
            local_base_key: self.local_base_key,
            global_bounds: Arc::new(OnceLock::new()),
        }
    }

    /// Rasterise the composed surface at the base grid's own resolution — for
    /// the consumers that genuinely need a grid (the static full-DEM mesh and
    /// collider). Detail below the grid's own spacing is Nyquist-gated out.
    /// Streaming consumers sample the oracle directly instead.
    pub fn materialize(&self) -> HeightGrid {
        let mut grid = (*self.base).clone();
        if self.modifiers.is_empty() {
            return grid;
        }
        let limited = self.detail_limited(self.spacing() as f64);
        let this = &limited;
        let res = grid.res;
        let s = grid.spacing();
        let origin = -grid.half_extent;
        for iz in 0..res {
            let z = (origin + iz as f32 * s) as f64;
            for ix in 0..res {
                let x = (origin + ix as f32 * s) as f64;
                let i = iz * res + ix;
                let mut h = grid.heights[i];
                for m in &this.modifiers {
                    h = m.apply(x, z, h);
                }
                grid.heights[i] = h;
            }
        }
        grid
    }
}

impl HeightSource for SurfaceOracle {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        let mut h = match &self.regional {
            Some(regional) => match regional.height_at(x, z) {
                Ok(height) => height,
                Err(error) => {
                    if !NON_FINITE_REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        bevy::log::error!(
                            "regional terrain sample rejected at ({x}, {z}): {error}"
                        );
                    }
                    return f64::NAN;
                }
            },
            None => HeightSource::height_at(self.base.as_ref(), x, z),
        };
        for m in &self.modifiers {
            h = m.apply(x, z, h);
        }
        // The oracle is the surface's SINGLE truth, so it is also the single
        // place a non-finite height may be caught — downstream (tile bake,
        // collider ring, obstacle grid) each guarding again would be four
        // copies of one invariant, and each would hide the origin a little
        // better than the last.
        //
        // A non-finite height is never a terrain fact: it means a non-finite
        // SAMPLE COORDINATE reached us (a poisoned frame upstream — see
        // `lunco-celestial`'s site anchor), or a modifier divided by an
        // authored zero. Report it once with the coordinate that identifies
        // the upstream culprit, then return the actual value. A fabricated
        // zero would turn a broken surface into apparently valid terrain.
        // Absurd-but-finite is reported too, and NOT substituted: it is a real
        // number the maths produced, so silently replacing it would hide the
        // modifier that produced it. `1e7` m is ~6× the Moon's radius — no
        // legitimate surface height reaches it.
        if h.is_finite()
            && h.abs() > 1.0e7
            && !ABSURD_REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            bevy::log::error!(
                "[terrain-oracle] absurd height {h:e} m at sample ({x}, {z}) \
                 — finite, so no guard catches it, but far outside any real \
                 surface. A modifier (curvature? over-zoom?) is producing it."
            );
        }
        if h.is_finite() {
            return h;
        }
        if !NON_FINITE_REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            bevy::log::error!(
                "[terrain-oracle] non-finite height at sample ({x}, {z}) — rejecting the sample. \
                 A non-finite SAMPLE COORDINATE (x/z above) means the terrain's frame was \
                 poisoned upstream; a finite coordinate means a modifier produced it."
            );
        }
        h
    }
}

/// One-shot latch for the oracle's non-finite report. A poisoned frame makes
/// EVERY sample non-finite, so an unlatched log would emit once per vertex per
/// tile per frame and bury the thing it is trying to show.
static NON_FINITE_REPORTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// As [`NON_FINITE_REPORTED`], for the absurd-magnitude report.
static ABSURD_REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

const MAX_SURFACE_RAY_CELL_VISITS: usize = 16_384;
const MAX_SURFACE_RAY_SUBDIVISIONS: usize = 32_768;
const MAX_SURFACE_RAY_SUBDIVISION_DEPTH: u8 = 48;
const SURFACE_RAY_DISTANCE_TOLERANCE_M: f64 = 0.005;

/// First intersection of a ray with the composed surface in terrain-local
/// coordinates. Raster cells are visited front-to-back. Conservative composed
/// height bounds reject clear intervals; possible intersections are subdivided
/// until bracketed or bounded to millimetre-scale distance. An unavailable
/// modifier bound or exhausted work budget is an error, not a false miss.
///
/// The returned point is on the analytic surface, which is also the ground
/// truth for editor picking. Render tiles and streamed physics colliders are
/// consumers of this oracle and need not be resident at the picked location.
pub fn raycast_surface(
    oracle: &SurfaceOracle,
    origin: bevy::math::DVec3,
    dir: bevy::math::DVec3,
    max_t: f64,
) -> Result<Option<bevy::math::DVec3>, String> {
    let Some(t) = raycast_surface_distance(oracle, origin, dir, max_t, 0.0)? else {
        return Ok(None);
    };
    let hit = origin + dir * t;
    Ok(Some(bevy::math::DVec3::new(
        hit.x,
        oracle.height_at(hit.x, hit.z),
        hit.z,
    )))
}

/// Distance to the first surface crossing after applying a non-negative height
/// margin. Used by line-of-sight queries that must not count a surface endpoint
/// as an occluder.
pub(crate) fn raycast_surface_distance(
    oracle: &SurfaceOracle,
    origin: bevy::math::DVec3,
    dir: bevy::math::DVec3,
    max_t: f64,
    margin: f64,
) -> Result<Option<f64>, String> {
    use bevy::math::DVec3;

    if !origin.is_finite()
        || !dir.is_finite()
        || (dir.length_squared() - 1.0).abs() > 1e-6
        || (!max_t.is_finite() && max_t != f64::INFINITY)
        || max_t <= 0.0
        || !margin.is_finite()
        || margin < 0.0
    {
        return Err("invalid terrain ray".into());
    }
    let half = oracle.domain_half_extent() as f64;
    let spacing = oracle.occlusion_spacing();
    let resolution = oracle.domain_resolution();
    if resolution < 2 || !spacing.is_finite() || spacing <= 0.0 {
        return Err("terrain ray has invalid raster geometry".into());
    }
    let inside = |p: DVec3| p.x.abs() <= half && p.z.abs() <= half;
    let above = |t: f64| {
        let p = origin + dir * t;
        p.y >= oracle.height_at(p.x, p.z) - margin
    };

    // A vertical ray stays in one raster cell. Resolve it directly and avoid an
    // unbounded march when callers request an infinite ray length.
    if dir.x.abs() <= f64::EPSILON && dir.z.abs() <= f64::EPSILON {
        if !inside(origin) || dir.y.abs() <= f64::EPSILON {
            return Ok(None);
        }
        let height = oracle.height_at(origin.x, origin.z) - margin;
        if origin.y < height {
            return Ok(None);
        }
        let t = (height - origin.y) / dir.y;
        return Ok((t >= 0.0 && t <= max_t).then_some(t));
    }

    // Intersect the ray with the finite DEM footprint before marching. Besides
    // avoiding pointless samples in the empty space before/after a terrain,
    // this makes an unbounded placement ray safe: the work is bounded by the
    // actual authored surface, not an arbitrary UI distance around a rover.
    let slab = |origin: f64, direction: f64| -> Option<(f64, f64)> {
        if direction.abs() <= f64::EPSILON {
            (origin.abs() <= half).then_some((f64::NEG_INFINITY, f64::INFINITY))
        } else {
            let a = (-half - origin) / direction;
            let b = (half - origin) / direction;
            Some((a.min(b), a.max(b)))
        }
    };
    let Some((x_near, x_far)) = slab(origin.x, dir.x) else {
        return Ok(None);
    };
    let Some((z_near, z_far)) = slab(origin.z, dir.z) else {
        return Ok(None);
    };
    let mut t = x_near.max(z_near).max(0.0);
    let max_t = x_far.min(z_far).min(max_t);
    if t > max_t {
        return Ok(None);
    }
    if !inside(origin + dir * t) {
        return Ok(None);
    }
    if !above(t) {
        return Ok(None);
    }

    let coordinate = |p: f64| ((p + half) / spacing).clamp(0.0, resolution as f64 - 1.0);
    let cell_index = |u: f64, step: isize| -> usize {
        let floor = u.floor();
        let at_boundary = (u - floor).abs() <= 32.0 * f64::EPSILON * u.abs().max(1.0);
        let raw = if step < 0 && at_boundary {
            floor as isize - 1
        } else {
            floor as isize
        };
        raw.clamp(0, resolution as isize - 2) as usize
    };
    let step_x = if dir.x > 0.0 {
        1_isize
    } else if dir.x < 0.0 {
        -1
    } else {
        0
    };
    let step_z = if dir.z > 0.0 {
        1_isize
    } else if dir.z < 0.0 {
        -1
    } else {
        0
    };
    let at = origin + dir * t;
    let mut ix = cell_index(coordinate(at.x), step_x);
    let mut iz = cell_index(coordinate(at.z), step_z);
    let mut next_x = if step_x == 0 {
        f64::INFINITY
    } else {
        let boundary = -half + (ix + usize::from(step_x > 0)) as f64 * spacing;
        (boundary - origin.x) / dir.x
    };
    let mut next_z = if step_z == 0 {
        f64::INFINITY
    } else {
        let boundary = -half + (iz + usize::from(step_z > 0)) as f64 * spacing;
        (boundary - origin.z) / dir.z
    };
    let delta_x = if step_x == 0 {
        f64::INFINITY
    } else {
        spacing / dir.x.abs()
    };
    let delta_z = if step_z == 0 {
        f64::INFINITY
    } else {
        spacing / dir.z.abs()
    };
    let mut cells = 0usize;
    let mut subdivisions = 0usize;
    let mut remaining_enclosures = MAX_SURFACE_RAY_SUBDIVISIONS;

    while t <= max_t {
        cells += 1;
        if cells > MAX_SURFACE_RAY_CELL_VISITS {
            return Err("terrain ray exceeded its raster-cell work budget".into());
        }
        let cell_end = next_x.min(next_z).min(max_t);
        if cell_end > t {
            // The bounded depth keeps this stack local and allocation-free; a
            // cursor ray can visit many raster cells in one presentation frame.
            let mut intervals = [(0.0, 0.0, 0u8); MAX_SURFACE_RAY_SUBDIVISION_DEPTH as usize + 2];
            let mut interval_count = 1usize;
            intervals[0] = (t, cell_end, 0);
            while interval_count > 0 {
                interval_count -= 1;
                let (lo, hi, depth) = intervals[interval_count];
                subdivisions += 1;
                if subdivisions > MAX_SURFACE_RAY_SUBDIVISIONS {
                    return Err("terrain ray exceeded its intersection work budget".into());
                }
                let p0 = origin + dir * lo;
                let p1 = origin + dir * hi;
                let rect = [
                    p0.x.min(p1.x),
                    p0.z.min(p1.z),
                    p0.x.max(p1.x),
                    p0.z.max(p1.z),
                ];
                let bounds =
                    oracle.height_bounds_in_cell([ix, iz], rect, &mut remaining_enclosures)?;
                let ray_low = p0.y.min(p1.y);
                let ray_high = p0.y.max(p1.y);
                let terrain_low = bounds[0] - margin;
                let terrain_high = bounds[1] - margin;
                if ray_low > terrain_high || ray_high < terrain_low {
                    continue;
                }

                let f0 = p0.y - oracle.height_at(p0.x, p0.z) + margin;
                let f1 = p1.y - oracle.height_at(p1.x, p1.z) + margin;
                if f0 >= 0.0 && f1 < 0.0 {
                    let (mut above_t, mut below_t) = (lo, hi);
                    for _ in 0..48 {
                        let mid = 0.5 * (above_t + below_t);
                        if above(mid) {
                            above_t = mid;
                        } else {
                            below_t = mid;
                        }
                    }
                    return Ok(Some(0.5 * (above_t + below_t)));
                }
                if f0 < 0.0 {
                    // The first in-footprint sample is below terrain; do not
                    // invent a hit by tracing back through the DEM boundary.
                    return Ok(None);
                }
                let vertical_uncertainty = ray_high - ray_low + bounds[1] - bounds[0];
                if hi - lo <= SURFACE_RAY_DISTANCE_TOLERANCE_M
                    && vertical_uncertainty <= SURFACE_RAY_DISTANCE_TOLERANCE_M
                {
                    return Ok(Some(0.5 * (lo + hi)));
                }
                if depth >= MAX_SURFACE_RAY_SUBDIVISION_DEPTH {
                    return Err("terrain ray could not certify a grazing intersection".into());
                }

                let mid = 0.5 * (lo + hi);
                let child_depth = depth + 1;
                // LIFO: push the far interval first so the nearest possible
                // crossing is always resolved before anything behind it.
                if interval_count + 2 > intervals.len() {
                    return Err("terrain ray exceeded its subdivision stack".into());
                }
                intervals[interval_count] = (mid, hi, child_depth);
                interval_count += 1;
                intervals[interval_count] = (lo, mid, child_depth);
                interval_count += 1;
            }
        }

        if cell_end >= max_t {
            break;
        }
        let epsilon = 16.0 * f64::EPSILON * cell_end.abs().max(1.0);
        if next_x <= cell_end + epsilon {
            ix = ix.checked_add_signed(step_x).unwrap_or(ix);
            next_x += delta_x;
        }
        if next_z <= cell_end + epsilon {
            iz = iz.checked_add_signed(step_z).unwrap_or(iz);
            next_z += delta_z;
        }
        t = cell_end;
        if ix >= resolution - 1 || iz >= resolution - 1 {
            break;
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use lunco_terrain_core::{Crater, Craters};

    #[test]
    fn global_height_bounds_enclose_bicubic_overshoot_and_ordered_modifiers() {
        #[derive(Clone)]
        struct Affine {
            scale: f64,
            offset: f64,
        }
        impl HeightModifier for Affine {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                height * self.scale + self.offset
            }
            fn height_bounds(&self, _: [f64; 4], input: [f64; 2]) -> Option<[f64; 2]> {
                let ends = input.map(|height| self.apply(0.0, 0.0, height));
                Some([ends[0].min(ends[1]), ends[0].max(ends[1])])
            }
        }
        let mut grid = HeightGrid::new_flat(5, 2.0);
        for row in grid.heights.chunks_exact_mut(5) {
            row.copy_from_slice(&[0.0, 1.0, 1.0, 0.0, 0.0]);
        }
        let base = Arc::new(grid);
        let bare = SurfaceOracle::bare(base.clone());
        let bounds = bare.global_height_bounds().unwrap();
        assert!(
            bounds[1] > 1.1,
            "a raw posting maximum misses bicubic overshoot: {bounds:?}"
        );
        let oracle = SurfaceOracle::new(
            base.clone(),
            vec![
                HeightContribution {
                    modifier: Arc::new(Affine {
                        scale: -2.0,
                        offset: 0.0,
                    }),
                    content_key: 1,
                },
                HeightContribution {
                    modifier: Arc::new(Affine {
                        scale: 1.0,
                        offset: 3.0,
                    }),
                    content_key: 2,
                },
            ],
        );
        let composed = oracle.global_height_bounds().unwrap();
        for z in 0..=64 {
            for x in 0..=64 {
                let x = -2.0 + x as f64 / 16.0;
                let z = -2.0 + z as f64 / 16.0;
                let raw = bare.height_at(x, z);
                let height = oracle.height_at(x, z);
                assert!(bounds[0] <= raw && raw <= bounds[1]);
                assert!(
                    composed[0] <= height && height <= composed[1],
                    "({x},{z}) {height} outside {composed:?}"
                );
            }
        }
        assert!((composed[0] - (3.0 - 2.0 * bounds[1])).abs() < 1e-12);
        assert!((composed[1] - (3.0 - 2.0 * bounds[0])).abs() < 1e-12);
        let reversed = SurfaceOracle::new(
            base,
            vec![
                HeightContribution {
                    modifier: Arc::new(Affine {
                        scale: 1.0,
                        offset: 3.0,
                    }),
                    content_key: 2,
                },
                HeightContribution {
                    modifier: Arc::new(Affine {
                        scale: -2.0,
                        offset: 0.0,
                    }),
                    content_key: 1,
                },
            ],
        );
        assert_ne!(composed, reversed.global_height_bounds().unwrap());
    }

    #[test]
    fn global_height_bounds_cache_shares_clones_and_resets_filtered_geometry() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Counted {
            height: f64,
            calls: Arc<AtomicUsize>,
        }
        impl HeightModifier for Counted {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                height + self.height
            }
            fn height_bounds(&self, _: [f64; 4], input: [f64; 2]) -> Option<[f64; 2]> {
                self.calls.fetch_add(1, Ordering::Relaxed);
                Some(input.map(|height| height + self.height))
            }
            fn with_min_wavelength(&self, _: f64) -> Option<Arc<dyn HeightModifier>> {
                Some(Arc::new(Self {
                    height: 2.0,
                    calls: self.calls.clone(),
                }))
            }
            fn for_region(
                &self,
                _: [f64; 2],
                _: [f64; 2],
                wavelength: f64,
            ) -> Option<Arc<dyn HeightModifier>> {
                self.with_min_wavelength(wavelength)
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let source = SurfaceOracle::new(
            flat(5, 2.0),
            vec![HeightContribution {
                modifier: Arc::new(Counted {
                    height: 1.0,
                    calls: calls.clone(),
                }),
                content_key: 1,
            }],
        );
        assert!(source.cached_global_height_bounds().is_none());
        let bounds = source.global_height_bounds().unwrap();
        let clone = source.clone();
        assert!(Arc::ptr_eq(&source.global_bounds, &clone.global_bounds));
        assert_eq!(clone.cached_global_height_bounds(), Some(Ok(bounds)));
        assert_eq!(clone.global_height_bounds().unwrap(), bounds);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        for filtered in [
            source.detail_limited(4.0),
            source.detail_limited_region(4.0, [-1.0; 2], [1.0; 2]),
        ] {
            assert!(!Arc::ptr_eq(&source.global_bounds, &filtered.global_bounds));
            assert!(filtered.cached_global_height_bounds().is_none());
            let filtered_bounds = filtered.global_height_bounds().unwrap();
            assert!(
                filtered_bounds[0] > bounds[1],
                "filtered geometry must not reuse the full-band proof"
            );
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn global_height_bounds_cover_registered_transition_and_full_modifier_stack() {
        use lunco_geotiff::{GeoTransform, LunarFrame};
        let make_grid = |res: usize, half: f32, offset: f64, slope: f64| {
            let mut grid = HeightGrid::new_flat(res, half);
            let spacing = f64::from(grid.spacing());
            for z in 0..res {
                for x in 0..res {
                    grid.heights[z * res + x] =
                        offset + slope * (-f64::from(half) + x as f64 * spacing);
                }
            }
            Arc::new(grid)
        };
        let geo = |half: f64, res: usize| {
            GeoTransform::centred_square(half * 2.0, res, 1_737_400.0, -84.0, 29.0)
                .with_frame(LunarFrame::MoonMe)
        };
        let regional = lunco_terrain_bake::regional::RegisteredRegionalSurface::new(
            make_grid(5, 2.0, 5.0, 0.5),
            geo(2.0, 5),
            make_grid(9, 8.0, -2.0, 0.1),
            geo(8.0, 9),
            3.0,
            7.0,
        )
        .unwrap();
        let source = SurfaceOracle::registered(
            Arc::new(regional),
            vec![
                crater_contribution(),
                curvature_contribution(1_737_400.0, 5.0),
            ],
        );
        let bounds = source.global_height_bounds().unwrap();
        for z in 0..=128 {
            for x in 0..=128 {
                let x = -8.0 + x as f64 / 8.0;
                let z = -8.0 + z as f64 / 8.0;
                let height = source.height_at(x, z);
                assert!(
                    bounds[0] <= height && height <= bounds[1],
                    "regional ({x},{z}) {height} outside {bounds:?}"
                );
            }
        }
    }

    #[test]
    fn global_height_bounds_reject_malformed_rasters_and_cache_missing_proofs() {
        for grid in [
            HeightGrid {
                res: 1,
                half_extent: 1.0,
                heights: vec![0.0],
            },
            HeightGrid {
                res: 3,
                half_extent: 1.0,
                heights: vec![0.0; 8],
            },
            HeightGrid {
                res: usize::MAX,
                half_extent: 1.0,
                heights: vec![],
            },
            HeightGrid {
                res: 2,
                half_extent: f32::NAN,
                heights: vec![0.0; 4],
            },
            HeightGrid {
                res: 2,
                half_extent: 1.0,
                heights: vec![f64::NAN; 4],
            },
        ] {
            let source = SurfaceOracle::new_with_base_key(Arc::new(grid), vec![], 1);
            assert!(
                source
                    .global_height_bounds()
                    .unwrap_err()
                    .contains("invalid terrain raster")
            );
        }
        struct Missing;
        impl HeightModifier for Missing {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                height
            }
        }
        let source = SurfaceOracle::new(
            flat(3, 1.0),
            vec![HeightContribution {
                modifier: Arc::new(Missing),
                content_key: 1,
            }],
        );
        let error = source.global_height_bounds().unwrap_err();
        assert!(error.contains("no conservative height bounds"));
        assert_eq!(source.clone().global_height_bounds().unwrap_err(), error);
        assert!(source.global_bounds.get().unwrap().is_err());
        struct Invalid([f64; 2]);
        impl HeightModifier for Invalid {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                height
            }
            fn height_bounds(&self, _: [f64; 4], _: [f64; 2]) -> Option<[f64; 2]> {
                Some(self.0)
            }
        }
        for bounds in [[2.0, 1.0], [0.0, f64::INFINITY]] {
            let source = SurfaceOracle::new(
                flat(3, 1.0),
                vec![HeightContribution {
                    modifier: Arc::new(Invalid(bounds)),
                    content_key: 2,
                }],
            );
            assert!(
                source
                    .global_height_bounds()
                    .unwrap_err()
                    .contains("invalid terrain height enclosure")
            );
        }
    }

    #[test]
    fn composed_affine_owner_rejects_inconsistent_modifier_certificates() {
        struct Inconsistent;
        impl HeightModifier for Inconsistent {
            fn apply(&self, _: f64, _: f64, height: f64) -> f64 {
                height
            }
            fn height_enclosure(&self, _: [f64; 4], _: HeightEnclosure) -> Option<HeightEnclosure> {
                Some(HeightEnclosure {
                    height: [1.0, 2.0],
                    plane: HeightPlane::horizontal(0.0),
                    residual: [0.0; 2],
                })
            }
        }
        let oracle = SurfaceOracle::new(
            Arc::new(HeightGrid::new_flat(5, 2.0)),
            vec![HeightContribution {
                modifier: Arc::new(Inconsistent),
                content_key: 1,
            }],
        );
        let mut budget = 8192;
        let error = oracle
            .height_enclosure_in_cell([2, 2], [0.1, 0.1, 0.4, 0.4], &mut budget)
            .unwrap_err();
        assert!(error.contains("invalid composed terrain affine enclosure"));
    }

    fn flat(res: usize, half: f32) -> Arc<HeightGrid> {
        Arc::new(HeightGrid::new_flat(res, half))
    }

    fn crater_contribution() -> HeightContribution {
        let craters = Craters::new(vec![Crater {
            center: [0.0, 0.0],
            radius: 10.0,
            depth: 2.0,
            rim_height: 0.4,
            softness: 0.0,
            bowl_power: 4.0,
        }]);
        HeightContribution {
            modifier: Arc::new(craters),
            content_key: 0xC0FFEE,
        }
    }

    #[test]
    fn bare_is_base() {
        let o = SurfaceOracle::bare(flat(9, 50.0));
        assert_eq!(HeightSource::height_at(&o, 3.0, -4.0), 0.0);
        assert_eq!(o.content_key(), 0);
        assert!(!o.has_modifiers());
    }

    #[test]
    fn modifiers_fold_over_base() {
        let o = SurfaceOracle::new(flat(9, 50.0), vec![crater_contribution()]);
        assert!(
            HeightSource::height_at(&o, 0.0, 0.0) < -1.0,
            "crater floor drops"
        );
        assert!(HeightSource::height_at(&o, 10.0, 0.0) > 0.0, "rim rises");
        assert_eq!(
            HeightSource::height_at(&o, 40.0, 40.0),
            0.0,
            "far field = base"
        );
        assert!(o.has_modifiers());
        assert_ne!(o.content_key(), 0);
    }

    #[test]
    fn site_curvature_preserves_dem_relief_and_grid() {
        let mut grid = HeightGrid::new_flat(9, 100.0);
        grid.heights.fill(-1_918.0);
        grid.heights[4 * grid.res + 4] = -1_888.0;
        grid.heights[4 * grid.res + 8] = -1_906.0;
        let base = Arc::new(grid);
        let raw_heights = base.heights.clone();
        let datum = base.border_datum();
        let radius_m = 1_737_400.0;
        let contributions = vec![curvature_contribution(radius_m, datum)];
        let oracle = SurfaceOracle::new(base.clone(), contributions);
        let sag = |x: f64, z: f64| {
            let site_radius = radius_m + datum;
            (site_radius * site_radius - x * x - z * z).sqrt() - site_radius
        };

        assert!((oracle.height_at(0.0, 0.0) + 1_888.0).abs() < 1.0e-9);
        assert_eq!(
            oracle.grid().heights,
            raw_heights,
            "the retained DEM is unchanged"
        );
        for (x, z) in [(50.0, 0.0), (99.0, 0.0), (100.0, 0.0), (-100.0, 40.0)] {
            let expected = HeightSource::height_at(base.as_ref(), x, z) + sag(x, z);
            assert!(
                (oracle.height_at(x, z) - expected).abs() < 1.0e-9,
                "cropped DEM relief changed at ({x}, {z})"
            );
        }
        let collider_surface = oracle.detail_limited_region(25.0, [-100.0, -100.0], [100.0, 100.0]);
        assert_eq!(
            collider_surface.height_at(100.0, 0.0),
            oracle.height_at(100.0, 0.0)
        );
        let materialized = oracle.materialize();
        assert_eq!(
            materialized.heights[4 * materialized.res + 8],
            oracle.height_at(100.0, 0.0)
        );
        assert_eq!(
            oracle.grid().heights,
            raw_heights,
            "materializing the derived surface must not mutate the DEM"
        );
        assert_ne!(
            oracle.surface_key(),
            SurfaceOracle::bare(flat(9, 100.0)).surface_key()
        );
    }

    #[test]
    fn materialize_matches_gated_sampling_at_grid_points() {
        let o = SurfaceOracle::new(flat(33, 50.0), vec![crater_contribution()]);
        let m = o.materialize();
        let s = m.spacing();
        // `materialize` Nyquist-gates at the grid's own spacing (craters included,
        // since they band-limit too) — so it must equal sampling the GATED oracle,
        // not the full-detail one.
        let limited = o.detail_limited(s as f64);
        for iz in 0..m.res {
            for ix in 0..m.res {
                let x = (-m.half_extent + ix as f32 * s) as f64;
                let z = (-m.half_extent + iz as f32 * s) as f64;
                let sampled = HeightSource::height_at(&limited, x, z);
                assert!(
                    (m.heights[iz * m.res + ix] - sampled).abs() < 1e-9,
                    "mismatch at ({x},{z})"
                );
            }
        }
    }

    #[test]
    fn content_key_deterministic_and_order_sensitive() {
        let a = SurfaceOracle::new(flat(5, 10.0), vec![crater_contribution()]);
        let b = SurfaceOracle::new(flat(5, 10.0), vec![crater_contribution()]);
        assert_eq!(a.content_key(), b.content_key());
    }

    #[test]
    fn raycast_surface_checks_the_end_of_a_short_segment() {
        let oracle = SurfaceOracle::bare(flat(5, 10.0));
        let hit = raycast_surface(&oracle, DVec3::new(0.0, 0.1, 0.0), DVec3::NEG_Y, 0.2)
            .expect("the ray must resolve")
            .expect("the surface crossing near the segment end must be detected");
        assert!(hit.y.abs() < 1e-6);
        assert!(
            raycast_surface(&oracle, DVec3::new(0.0, 0.1, 0.0), DVec3::NEG_Y, 0.05)
                .expect("the short ray must resolve")
                .is_none()
        );
    }

    #[test]
    fn raycast_surface_reaches_a_distant_finite_dem_with_an_unbounded_ray() {
        let oracle = SurfaceOracle::bare(flat(5, 10.0));
        // The ray starts a kilometre before the DEM window and crosses the
        // flat surface at its centre. This is the placement-ghost case that
        // previously stopped at a fixed near-rover distance.
        let origin = DVec3::new(-1_000.0, 100.0, 0.0);
        let direction = DVec3::new(1.0, -0.1, 0.0).normalize();

        let hit = raycast_surface(&oracle, origin, direction, f64::INFINITY)
            .expect("the finite terrain footprint bounds an unbounded ray")
            .expect("the ray must intersect the DEM");
        assert!(hit.x.abs() < 0.1, "expected centre hit, got {hit:?}");
        assert!(hit.y.abs() < 1e-6, "expected flat ground, got {hit:?}");
    }

    #[test]
    fn raycast_surface_does_not_step_over_a_narrow_ridge() {
        let mut grid = HeightGrid::new_flat(129, 32.0);
        grid.heights[64 * grid.res + 64] = 4.0;
        let oracle = SurfaceOracle::bare(Arc::new(grid));
        let hit = raycast_surface(&oracle, DVec3::new(-20.0, 1.0, 0.0), DVec3::X, 40.0)
            .expect("bounded cell traversal must resolve the ridge")
            .expect("the ray must hit the narrow ridge");
        assert!(
            hit.x > -1.0 && hit.x < 0.0,
            "expected near ridge face, got {hit:?}"
        );
        assert!(
            (hit.y - oracle.height_at(hit.x, hit.z)).abs() < 1e-9,
            "hit must lie on the composed surface, got {hit:?}"
        );
    }

    #[test]
    fn raycast_surface_rejects_invalid_and_uncertifiable_queries() {
        let base = SurfaceOracle::bare(flat(5, 10.0));
        let invalid = raycast_surface(&base, DVec3::new(0.0, 1.0, 0.0), DVec3::ZERO, 10.0)
            .expect_err("a non-unit direction must be rejected");
        assert!(invalid.contains("invalid terrain ray"), "{invalid}");

        struct UnboundedModifier;
        impl HeightModifier for UnboundedModifier {
            fn apply(&self, _x: f64, _z: f64, height: f64) -> f64 {
                height
            }
        }

        let unbounded = SurfaceOracle::new(
            flat(5, 10.0),
            vec![HeightContribution {
                modifier: Arc::new(UnboundedModifier),
                content_key: 1,
            }],
        );
        let error = raycast_surface(
            &unbounded,
            DVec3::new(0.0, 1.0, 0.0),
            DVec3::new(1.0, -1.0, 0.0).normalize(),
            10.0,
        )
        .expect_err("a modifier without height bounds cannot certify a ray");
        assert!(error.contains("no conservative height bounds"), "{error}");
    }
}
