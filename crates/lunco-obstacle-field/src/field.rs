//! The generated height surface.
//!
//! Since the rover currently drives on a flat slab, craters can't be "stamped
//! into existing terrain" — so we synthesise the height array ourselves: start
//! flat, write a bowl profile per crater, then hand the array to Avian as a
//! heightfield collider and to Bevy as a visual mesh. The same array gives an
//! analytic `height_at(x, z)`, so rock placement resolves ground height off the
//! main thread with no raycasts.

use bevy::math::Vec2;

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
                let bowl = if d < 1.0 { -depth * (1.0 - d * d) } else { 0.0 };
                // Raised wall centred on the rim (d = 1), Gaussian falloff.
                let rim = rim_height * (-((d - 1.0) / 0.28).powi(2)).exp();
                let delta = (bowl + rim) as f64;
                let i = self.idx(ix as usize, iz as usize);
                self.heights[i] += delta;
            }
        }
    }

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

    /// Build visual mesh vertex data (positions in metres, smooth normals from
    /// central differences, UVs across the region).
    pub fn to_mesh_data(&self) -> MeshData {
        let res = self.res;
        let mut positions = Vec::with_capacity(res * res);
        let mut normals = Vec::with_capacity(res * res);
        let mut uvs = Vec::with_capacity(res * res);
        let s = self.spacing();

        for iz in 0..res {
            for ix in 0..res {
                let p = self.sample_pos(ix, iz);
                let y = self.heights[self.idx(ix, iz)] as f32;
                positions.push([p.x, y, p.y]);
                uvs.push([ix as f32 / (res as f32 - 1.0), iz as f32 / (res as f32 - 1.0)]);

                // Central-difference normal.
                let hl = self.heights[self.idx(ix.saturating_sub(1), iz)] as f32;
                let hr = self.heights[self.idx((ix + 1).min(res - 1), iz)] as f32;
                let hd = self.heights[self.idx(ix, iz.saturating_sub(1))] as f32;
                let hu = self.heights[self.idx(ix, (iz + 1).min(res - 1))] as f32;
                let n = bevy::math::Vec3::new(hl - hr, 2.0 * s, hd - hu).normalize_or_zero();
                normals.push([n.x, n.y, n.z]);
            }
        }

        let mut indices = Vec::with_capacity((res - 1) * (res - 1) * 6);
        for iz in 0..res - 1 {
            for ix in 0..res - 1 {
                let i = (iz * res + ix) as u32;
                let r = i + res as u32;
                indices.extend_from_slice(&[i, r, i + 1, i + 1, r, r + 1]);
            }
        }

        MeshData { positions, normals, uvs, indices }
    }
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
