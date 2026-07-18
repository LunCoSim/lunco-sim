//! The generated height surface.
//!
//! Since the rover currently drives on a flat slab, craters can't be "stamped
//! into existing terrain" — so we synthesise the height array ourselves: start
//! flat, write a bowl profile per crater, then hand the array to Avian as a
//! heightfield collider and to Bevy as a visual mesh. The same array gives an
//! analytic `height_at(x, z)`, so rock placement resolves ground height off the
//! main thread with no raycasts.

use bevy::math::Vec2;
use lunco_terrain_core::HeightSource;

use crate::sampler::Placement;
use crate::spec::CraterLayer;

/// A square, origin-centred height grid. `heights` is row-major, indexed
/// `[z * res + x]`, spanning `[-half_extent, half_extent]` on both axes.
#[derive(Clone, Debug)]
pub struct HeightGrid {
    pub res: usize,
    pub half_extent: f32,
    pub heights: Vec<f64>,
}

/// Interleaved vertex data for a visual terrain mesh (no bevy_mesh dependency
/// here — the plugin assembles these into a `Mesh`).
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl HeightGrid {
    pub fn new_flat(res: usize, half_extent: f32) -> Self {
        let res = res.max(2);
        Self { res, half_extent, heights: vec![0.0; res * res] }
    }

    /// Metres between adjacent samples.
    pub fn spacing(&self) -> f32 {
        (2.0 * self.half_extent) / (self.res as f32 - 1.0)
    }

    #[inline]
    fn idx(&self, x: usize, z: usize) -> usize {
        z * self.res + x
    }

    /// World position (XZ) of sample `(ix, iz)`.
    fn sample_pos(&self, ix: usize, iz: usize) -> Vec2 {
        let s = self.spacing();
        Vec2::new(-self.half_extent + ix as f32 * s, -self.half_extent + iz as f32 * s)
    }

    /// Additively stamp one crater. Profile: parabolic bowl inside the rim plus a
    /// Gaussian raised lip at the rim. Craters accumulate (overlap naturally).
    pub fn stamp_crater(&mut self, center: Vec2, radius: f32, depth: f32, rim_height: f32) {
        if radius <= 0.0 {
            return;
        }
        let s = self.spacing();
        let reach = radius * 1.6; // bowl + rim falloff
        // Bounding box of affected samples (clamped to grid).
        let to_i = |w: f32| -> i32 { ((w + self.half_extent) / s).round() as i32 };
        let min_x = to_i(center.x - reach).max(0);
        let max_x = to_i(center.x + reach).min(self.res as i32 - 1);
        let min_z = to_i(center.y - reach).max(0);
        let max_z = to_i(center.y + reach).min(self.res as i32 - 1);

        for iz in min_z..=max_z {
            for ix in min_x..=max_x {
                let p = self.sample_pos(ix as usize, iz as usize);
                let d = p.distance(center) / radius; // normalised radial distance
                let i = self.idx(ix as usize, iz as usize);
                self.heights[i] += crater_delta(d, depth, rim_height) as f64;
            }
        }
    }

    // (crater cross-section: `crater_delta` below — an f32 wrapper over the
    // canonical `lunco_terrain_core::crater_profile`. Streamed tiles sample
    // terrain-core's crater layer directly; this stamp path serves only
    // Standalone-mode obstacle fields.)

    /// Stamp every crater placement using the layer's depth/rim ratios.
    pub fn stamp_craters(&mut self, placements: &[Placement], layer: &CraterLayer) {
        for p in placements {
            let depth = p.size * layer.depth_ratio;
            let rim = depth * layer.rim_height_ratio;
            self.stamp_crater(p.pos, p.size, depth, rim);
        }
    }

    /// Height sample at integer grid coords. The cubic needs one sample either
    /// side of the interpolated cell, so it reaches up to 1 before and 2 past the
    /// grid; out-of-range indices are **linearly extrapolated** from the two
    /// outermost rows.
    ///
    /// Not clamped. Clamping duplicates the edge row, which reads to the cubic as
    /// the surface abruptly levelling off, so the interpolant bends near every
    /// border — it stamps a faint rim around the whole DEM and stops reproducing
    /// even a flat ramp there. Extrapolation continues the existing trend instead,
    /// which keeps a plane exact right to the edge.
    #[inline]
    fn sample_extrapolated(&self, ix: isize, iz: isize) -> f64 {
        let last = self.res as isize - 1;
        // Resolve each axis to an in-range anchor plus how far outside it lies.
        let (ax, dx) = if ix < 0 {
            (0isize, ix)
        } else if ix > last {
            (last, ix - last)
        } else {
            (ix, 0)
        };
        let (az, dz) = if iz < 0 {
            (0isize, iz)
        } else if iz > last {
            (last, iz - last)
        } else {
            (iz, 0)
        };
        // Inward neighbour on each axis, used for the outward slope.
        let inx = if dx < 0 { 1 } else if dx > 0 { last - 1 } else { ax };
        let inz = if dz < 0 { 1 } else if dz > 0 { last - 1 } else { az };
        let at = |x: isize, z: isize| self.heights[self.idx(x as usize, z as usize)];

        let h = at(ax, az);
        let mut out = h;
        if dx != 0 {
            out += dx.unsigned_abs() as f64 * (h - at(inx, az));
        }
        if dz != 0 {
            out += dz.unsigned_abs() as f64 * (h - at(ax, inz));
        }
        out
    }

    /// Catmull-Rom cubic through `p1`/`p2`, using `p0`/`p3` to set the end slopes.
    ///
    /// Chosen over a B-spline because it is **interpolating**: at `t = 0` it is
    /// exactly `p1` and at `t = 1` exactly `p2`. The DEM's measured heights are
    /// therefore reproduced bit-for-bit at every post — only the values *between*
    /// posts change — so colliders, queries and visuals keep agreeing on the same
    /// surface and no physics behaviour shifts under this.
    #[inline]
    fn catmull_rom(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
        let t2 = t * t;
        let t3 = t2 * t;
        0.5 * ((2.0 * p1)
            + (-p0 + p2) * t
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
            + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
    }

    /// Bicubic (Catmull-Rom) ground height at world `(x, z)`, clamped to region.
    ///
    /// This was bilinear, and bilinear is only C0: its gradient is *constant
    /// within each cell* and jumps at cell borders. Normals are probed by finite
    /// difference at a fixed sub-metre `eps` (`tile_mesh`), so every probe inside
    /// one ~4 m DEM cell returned the SAME normal — the terrain shaded as flat
    /// facets one DEM cell across, the "blocky terrain" artifact. Widening `eps`
    /// to the cell size hid the facets but erased the sub-cell relief the analytic
    /// overzoom layer synthesises, which is why that fix was rejected.
    ///
    /// Catmull-Rom is C1, so the gradient varies continuously across cell borders
    /// and a fine probe reports a genuinely different normal at each vertex. The
    /// cure is in the interpolant, where the discontinuity actually lives, rather
    /// than in the probe that merely revealed it.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let s = self.spacing();
        let fx = ((x + self.half_extent) / s).clamp(0.0, self.res as f32 - 1.0);
        let fz = ((z + self.half_extent) / s).clamp(0.0, self.res as f32 - 1.0);
        let x0 = fx.floor() as isize;
        let z0 = fz.floor() as isize;
        let tx = (fx - x0 as f32) as f64;
        let tz = (fz - z0 as f32) as f64;

        // Four rows through z-1..z+2, each collapsed along x, then across in z.
        let mut rows = [0.0f64; 4];
        for (r, row) in rows.iter_mut().enumerate() {
            let iz = z0 + r as isize - 1;
            *row = Self::catmull_rom(
                self.sample_extrapolated(x0 - 1, iz),
                self.sample_extrapolated(x0, iz),
                self.sample_extrapolated(x0 + 1, iz),
                self.sample_extrapolated(x0 + 2, iz),
                tx,
            );
        }
        Self::catmull_rom(rows[0], rows[1], rows[2], rows[3], tz) as f32
    }

    /// Convert to Avian's heightfield layout: `Vec<Vec<f64>>` indexed `[x][z]`,
    /// paired with a `DVec3` scale of `(width, 1, depth)`. Parry centres the
    /// heightfield at the origin, matching our `[-h, h]` region.
    pub fn to_avian_heights(&self) -> Vec<Vec<f64>> {
        let mut out = vec![vec![0.0f64; self.res]; self.res];
        for z in 0..self.res {
            for x in 0..self.res {
                out[x][z] = self.heights[self.idx(x, z)];
            }
        }
        out
    }

    /// Build visual mesh vertex data (positions in metres, smooth normals, UVs
    /// across the region). Normals + indices come from the shared [`grid_normals`]
    /// / [`grid_indices`] helpers so terrain meshes are built identically here and
    /// in the terrain-streaming tile baker.
    pub fn to_mesh_data(&self) -> MeshData {
        let res = self.res;
        let mut positions = Vec::with_capacity(res * res);
        let mut uvs = Vec::with_capacity(res * res);
        for iz in 0..res {
            for ix in 0..res {
                let p = self.sample_pos(ix, iz);
                let y = self.heights[self.idx(ix, iz)] as f32;
                positions.push([p.x, y, p.y]);
                uvs.push([ix as f32 / (res as f32 - 1.0), iz as f32 / (res as f32 - 1.0)]);
            }
        }
        let normals = grid_normals(&positions, res);
        let indices = grid_indices(res);
        MeshData { positions, normals, uvs, indices }
    }
}

/// Height delta (m) of a simple bowl crater at normalised radial distance `d`
/// (0 = centre, 1 = rim radius). Shared by [`HeightGrid::stamp_crater`] (rasterised
/// into the streamed-tile grid) and the dedicated high-fidelity crater mesh
/// (`lunco-terrain-surface`'s craters layer), so both agree on the cross-section.
///
/// Reads as a real impact, not a soft saucer: a fairly flat floor (`1 - d⁴` stays
/// near max depth across the floor) turning UP into a steep inner wall, a SHARP
/// raised rim lip at `d≈1` (the key cue under raking light), then a low outward
/// ejecta apron to ~1.5 r.
pub fn crater_delta(d: f32, depth: f32, rim_height: f32) -> f32 {
    // Delegate to the canonical `f64` profile in `lunco-terrain-core` so the
    // rasterised stamp here and the analytic `CraterField` sampled by the tile
    // baker + collider share ONE cross-section — no second copy to drift. The
    // legacy quartic bowl (flat floor, steep wall band) is this stamp path's
    // fixed shape; the analytic layer varies `bowl_power` per crater instead.
    lunco_terrain_core::crater_profile(d as f64, depth as f64, rim_height as f64, 4.0) as f32
}

/// Smooth normals for a row-major `res×res` vertex grid via central differences
/// over each vertex's **actual** XZ spacing (one-sided at the edges). Shared by
/// [`HeightGrid::to_mesh_data`] and the terrain-streaming tile baker so terrain
/// normals are computed identically everywhere.
pub fn grid_normals(positions: &[[f32; 3]], res: usize) -> Vec<[f32; 3]> {
    let idx = |x: usize, z: usize| z * res + x;
    let mut normals = vec![[0.0, 1.0, 0.0]; res * res];
    for z in 0..res {
        for x in 0..res {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(res - 1);
            let zm = z.saturating_sub(1);
            let zp = (z + 1).min(res - 1);
            let hl = positions[idx(xm, z)][1];
            let hr = positions[idx(xp, z)][1];
            let hd = positions[idx(x, zm)][1];
            let hu = positions[idx(x, zp)][1];
            let dx = positions[idx(xp, z)][0] - positions[idx(xm, z)][0];
            let dz = positions[idx(x, zp)][2] - positions[idx(x, zm)][2];
            let nx = if dx != 0.0 { -(hr - hl) / dx } else { 0.0 };
            let nz = if dz != 0.0 { -(hu - hd) / dz } else { 0.0 };
            let len = (nx * nx + 1.0 + nz * nz).sqrt();
            normals[idx(x, z)] = [nx / len, 1.0 / len, nz / len];
        }
    }
    normals
}

/// A `HeightGrid` is a [`HeightSource`]: reuse its bilinear sampler, widening to
/// the trait's `f64` interface. The impl lives here (with the type) so the
/// foreign trait `lunco_terrain_core::HeightSource` is implemented for the local
/// `HeightGrid` — satisfying the orphan rule — and the planar streamer + any
/// other consumer can treat a loaded DEM as a generic height source.
impl HeightSource for HeightGrid {
    fn height_at(&self, x: f64, z: f64) -> f64 {
        HeightGrid::height_at(self, x as f32, z as f32) as f64
    }
}

/// Two CCW triangles per quad over a row-major `res×res` vertex grid.
pub fn grid_indices(res: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity((res - 1) * (res - 1) * 6);
    let row = res as u32;
    for iz in 0..(res as u32 - 1) {
        for ix in 0..(res as u32 - 1) {
            let i = iz * row + ix;
            indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    indices
}

/// Build the full height grid for a field: flat base + all craters stamped.
pub fn build_height_grid(
    res: usize,
    half_extent: f32,
    crater_placements: &[Placement],
    craters: &CraterLayer,
) -> HeightGrid {
    let mut grid = HeightGrid::new_flat(res, half_extent);
    if craters.enabled {
        grid.stamp_craters(crater_placements, craters);
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_zero() {
        let g = HeightGrid::new_flat(65, 50.0);
        assert_eq!(g.height_at(0.0, 0.0), 0.0);
        assert_eq!(g.height_at(50.0, -50.0), 0.0);
    }

    #[test]
    fn crater_center_is_depressed() {
        let mut g = HeightGrid::new_flat(257, 50.0);
        g.stamp_crater(Vec2::ZERO, 10.0, 2.0, 0.3);
        let center = g.height_at(0.0, 0.0);
        let outside = g.height_at(40.0, 40.0);
        assert!(center < -1.0, "center {center} should be a deep depression");
        assert!(outside.abs() < 1e-3, "far field {outside} should stay flat");
        // Rim lip is raised above the surrounding plain.
        let rim = g.height_at(10.0, 0.0);
        assert!(rim > 0.0, "rim {rim} should be raised");
    }

    /// A tilted plane, so every interpolant should reproduce it EXACTLY: cubic
    /// interpolation of collinear samples is that same line.
    fn ramp(res: usize, half: f32, slope: f64) -> HeightGrid {
        let mut g = HeightGrid::new_flat(res, half);
        let s = g.spacing() as f64;
        for iz in 0..res {
            for ix in 0..res {
                let x = -half as f64 + ix as f64 * s;
                let i = g.idx(ix, iz);
                g.heights[i] = x * slope;
            }
        }
        g
    }

    /// The interpolant must pass THROUGH the DEM's measured samples. This is what
    /// makes the bilinear -> Catmull-Rom swap safe for physics: colliders and
    /// queries still see the same heights at every post, so nothing that rests on
    /// or collides with the terrain moves.
    #[test]
    fn interpolation_reproduces_the_samples_exactly() {
        let mut g = HeightGrid::new_flat(33, 16.0);
        g.stamp_crater(Vec2::new(3.0, -2.0), 7.0, 2.5, 0.4);
        let s = g.spacing();
        for iz in 0..g.res {
            for ix in 0..g.res {
                let x = -g.half_extent + ix as f32 * s;
                let z = -g.half_extent + iz as f32 * s;
                let want = g.heights[g.idx(ix, iz)] as f32;
                let got = g.height_at(x, z);
                assert!(
                    (got - want).abs() < 1e-3,
                    "post ({ix},{iz}): interpolant {got} != sample {want}"
                );
            }
        }
    }

    /// Exactness on a plane — guards against a mis-signed Catmull-Rom coefficient,
    /// which would still look plausible on noisy data.
    #[test]
    fn a_plane_is_reproduced_between_samples_too() {
        let g = ramp(33, 16.0, 0.25);
        for k in 0..40 {
            let x = -12.0 + k as f32 * 0.6;
            assert!(
                (g.height_at(x, 1.7) as f64 - x as f64 * 0.25).abs() < 1e-4,
                "ramp not reproduced at x={x}"
            );
        }
    }

    /// The cubic reaches outside the grid near every border, so the edge rule is
    /// load-bearing. Clamping (duplicating the edge row) bends the interpolant
    /// there and stamps a faint rim around the DEM; linear extrapolation keeps the
    /// ramp exact right up to the boundary.
    ///
    /// A tiny grid is deliberate — at res=3 EVERY cell is a boundary cell, which
    /// is exactly the case that caught this.
    #[test]
    fn a_plane_survives_the_dem_boundary() {
        let g = ramp(3, 10.0, 0.1);
        for k in 0..21 {
            let x = -10.0 + k as f32; // includes both extreme edges
            let got = g.height_at(x, 0.0) as f64;
            assert!(
                (got - x as f64 * 0.1).abs() < 1e-5,
                "boundary bends the plane at x={x}: {got} != {}",
                x as f64 * 0.1
            );
        }
    }

    /// THE REGRESSION TEST for blocky terrain.
    ///
    /// Bilinear is C0: within a single cell its x-gradient does not depend on x at
    /// all, so a sub-cell finite-difference normal probe returns an identical
    /// normal at every vertex in that cell and the surface shades as one flat
    /// facet. Catmull-Rom is C1, so the gradient genuinely varies across the cell.
    ///
    /// Probing at 0.5 m inside one cell mirrors what `tile_mesh` actually does.
    #[test]
    fn gradient_varies_within_a_single_cell() {
        let mut g = HeightGrid::new_flat(65, 128.0);
        // Curvature is required: on a plane every interpolant is correctly linear.
        g.stamp_crater(Vec2::ZERO, 60.0, 8.0, 1.0);
        let s = g.spacing();
        assert!(s > 2.0, "cell {s} m must be wider than the probe for this to bite");

        // Two probe points inside the SAME cell, away from the crater centre where
        // the profile is curved.
        let cell_x = (30.0f32 / s).floor() * s - g.half_extent + g.half_extent;
        let base = (30.0f32 / s).floor() * s;
        let eps = 0.5f32;
        let grad_at = |x: f32| {
            (g.height_at(x + eps, 0.0) - g.height_at(x - eps, 0.0)) as f64 / (2.0 * eps) as f64
        };
        let _ = cell_x;
        let g1 = grad_at(base + 0.25 * s);
        let g2 = grad_at(base + 0.75 * s);
        assert!(
            (g1 - g2).abs() > 1e-4,
            "gradient is constant across the cell ({g1} vs {g2}) — the interpolant \
             is C0 and terrain will shade as flat per-cell facets"
        );
    }

    #[test]
    fn avian_layout_matches_height_at() {
        let mut g = HeightGrid::new_flat(33, 16.0);
        g.stamp_crater(Vec2::new(4.0, -4.0), 6.0, 3.0, 0.0);
        let av = g.to_avian_heights();
        // av[x][z] must equal the row-major sample.
        for z in 0..g.res {
            for x in 0..g.res {
                assert_eq!(av[x][z], g.heights[z * g.res + x]);
            }
        }
    }

    #[test]
    fn mesh_data_well_formed() {
        let g = HeightGrid::new_flat(17, 10.0);
        let m = g.to_mesh_data();
        assert_eq!(m.positions.len(), 17 * 17);
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.indices.len(), 16 * 16 * 6);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
    }
}
