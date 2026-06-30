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

    // (crater cross-section moved to the free fn `crater_delta` so the streamed-tile
    // stamp AND a dedicated high-fidelity crater mesh share one profile.)

    /// Stamp every crater placement using the layer's depth/rim ratios.
    pub fn stamp_craters(&mut self, placements: &[Placement], layer: &CraterLayer) {
        for p in placements {
            let depth = p.size * layer.depth_ratio;
            let rim = depth * layer.rim_height_ratio;
            self.stamp_crater(p.pos, p.size, depth, rim);
        }
    }

    /// Bilinear-interpolated ground height at world `(x, z)`, clamped to region.
    pub fn height_at(&self, x: f32, z: f32) -> f32 {
        let s = self.spacing();
        let fx = ((x + self.half_extent) / s).clamp(0.0, self.res as f32 - 1.0);
        let fz = ((z + self.half_extent) / s).clamp(0.0, self.res as f32 - 1.0);
        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(self.res - 1);
        let z1 = (z0 + 1).min(self.res - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let h00 = self.heights[self.idx(x0, z0)] as f32;
        let h10 = self.heights[self.idx(x1, z0)] as f32;
        let h01 = self.heights[self.idx(x0, z1)] as f32;
        let h11 = self.heights[self.idx(x1, z1)] as f32;
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * tz
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
    let bowl = if d < 1.0 { -depth * (1.0 - d * d * d * d) } else { 0.0 };
    let rim = rim_height * (-((d - 0.98) / 0.14).powi(2)).exp();
    let apron = if (1.0..1.6).contains(&d) {
        rim_height * 0.25 * (-((d - 1.15) / 0.30).powi(2)).exp()
    } else {
        0.0
    };
    bowl + rim + apron
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
