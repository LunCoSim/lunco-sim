//! Per-tile CDLOD mesh bake (milestone S2a, approach C).
//!
//! Each quadtree node bakes a real grid mesh from the DEM. Every vertex carries
//! two positions: its **own** LOD position (`POSITION`, `y = DEM height`) and a
//! **`MORPH_TARGET`** — the same vertex snapped to the parent node's coarser
//! (even) lattice with `y` re-sampled there. The CDLOD vertex shader lerps
//! `pos = mix(POSITION, MORPH_TARGET, morph)` by a camera-distance morph factor,
//! so a tile geomorphs smoothly into its parent with no popping and no
//! texture fetch. The same baked grid also yields the avian collider heights
//! (`HeightGrid::to_avian_heights`), so one bake feeds visuals *and* physics and
//! becomes one cached `TerrainTile` (see `docs/terrain-streaming-IMPL.md`).
//!
//! Pure + Bevy-free → unit-tested and wasm-safe; the plugin runs it off-thread
//! and assembles the attributes into a Bevy `Mesh`.

use lunco_obstacle_field::field::HeightGrid;

use crate::quadtree::Square;

/// CPU vertex data for one CDLOD tile. `morph_targets[i]` is the parent-lattice
/// position vertex `i` collapses to as the camera recedes. Positions are in
/// **world** XZ (S2 static; S3 rebases to a per-tile `CellCoord` local frame).
#[derive(Debug, Clone, PartialEq)]
pub struct TileMesh {
    pub positions: Vec<[f32; 3]>,
    pub morph_targets: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Bake a `res × res`-vertex CDLOD mesh covering `region`, sampling heights from
/// `dem`. `res` is clamped ≥ 2. `MORPH_TARGET` snaps each vertex index down to the
/// parent's even lattice (`idx & !1`) and re-samples the DEM there — so even
/// vertices don't move and odd vertices collapse onto their even neighbour, the
/// standard CDLOD vertex morph. UVs are DEM-global (`(world + H)/(2H)`) so layer
/// maps align across tiles. `dem_half_extent` is the DEM's `half_extent`.
pub fn bake_tile_mesh(dem: &HeightGrid, region: Square, res: usize, dem_half_extent: f64) -> TileMesh {
    let res = res.max(2);
    let n = res as f64;
    let step = region.side() / (n - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let inv_uv = 1.0 / (2.0 * dem_half_extent);

    let world = |ix: usize, iz: usize| -> (f64, f64) {
        (x0 + ix as f64 * step, z0 + iz as f64 * step)
    };
    let height = |wx: f64, wz: f64| -> f32 { dem.height_at(wx as f32, wz as f32) };

    let mut positions = Vec::with_capacity(res * res);
    let mut morph_targets = Vec::with_capacity(res * res);
    let mut uvs = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (wx, wz) = world(ix, iz);
            positions.push([wx as f32, height(wx, wz), wz as f32]);

            // Snap to the parent's even lattice and re-sample.
            let (sx, sz) = world(ix & !1, iz & !1);
            morph_targets.push([sx as f32, height(sx, sz), sz as f32]);

            uvs.push([((wx + dem_half_extent) * inv_uv) as f32, ((wz + dem_half_extent) * inv_uv) as f32]);
        }
    }

    let normals = grid_normals(&positions, res);
    let indices = grid_indices(res);
    TileMesh { positions, morph_targets, normals, uvs, indices }
}

/// Smooth normals from central differences over the grid (own positions).
fn grid_normals(positions: &[[f32; 3]], res: usize) -> Vec<[f32; 3]> {
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
            // Gradient → normal (y up). Guard against zero spacing at edges.
            let nx = if dx != 0.0 { -(hr - hl) / dx } else { 0.0 };
            let nz = if dz != 0.0 { -(hu - hd) / dz } else { 0.0 };
            let len = (nx * nx + 1.0 + nz * nz).sqrt();
            normals[idx(x, z)] = [nx / len, 1.0 / len, nz / len];
        }
    }
    normals
}

/// Two CCW triangles per quad over a `res × res` grid.
fn grid_indices(res: usize) -> Vec<u32> {
    let row = res as u32;
    let mut indices = Vec::with_capacity((res - 1) * (res - 1) * 6);
    for iz in 0..(res as u32 - 1) {
        for ix in 0..(res as u32 - 1) {
            let i = iz * row + ix;
            indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_dem() -> HeightGrid {
        HeightGrid { res: 8, half_extent: 100.0, heights: vec![0.0; 64] }
    }

    /// A DEM whose height equals its world X (a pure ramp in X).
    fn ramp_dem() -> HeightGrid {
        let res = 9;
        let half = 100.0f32;
        let s = (2.0 * half) / (res as f32 - 1.0);
        let mut heights = vec![0.0f64; res * res];
        for z in 0..res {
            for x in 0..res {
                heights[z * res + x] = (-half + x as f32 * s) as f64; // = world x
            }
        }
        HeightGrid { res, half_extent: half, heights }
    }

    #[test]
    fn flat_dem_bakes_flat_no_morph() {
        let m = bake_tile_mesh(&flat_dem(), Square { center: [0.0, 0.0], half: 50.0 }, 5, 100.0);
        assert_eq!(m.positions.len(), 25);
        assert_eq!(m.indices.len(), 4 * 4 * 6);
        assert!(m.positions.iter().all(|p| p[1] == 0.0));
        // Flat → morph target equals position (same height everywhere).
        assert!(m.normals.iter().all(|n| n[1] > 0.99));
    }

    #[test]
    fn even_vertices_do_not_move_odd_collapse_to_even() {
        let dem = ramp_dem();
        let region = Square { center: [0.0, 0.0], half: 50.0 };
        let res = 5;
        let m = bake_tile_mesh(&dem, region, res, 100.0);
        let step = region.side() / (res as f64 - 1.0);
        let x0 = region.center[0] - region.half;
        for iz in 0..res {
            for ix in 0..res {
                let i = iz * res + ix;
                if ix % 2 == 0 {
                    // Even vertex: morph target X == own X (no lateral move).
                    assert!((m.morph_targets[i][0] - m.positions[i][0]).abs() < 1e-3, "even vtx moved");
                } else {
                    // Odd vertex: collapses to the lower even neighbour's X.
                    let snapped_x = (x0 + (ix & !1) as f64 * step) as f32;
                    assert!((m.morph_targets[i][0] - snapped_x).abs() < 1e-3);
                    // On an X-ramp, morphed height = snapped world X.
                    assert!((m.morph_targets[i][1] - snapped_x).abs() < 1e-2, "morph height wrong");
                }
            }
        }
    }

    #[test]
    fn positions_carry_dem_height() {
        let dem = ramp_dem();
        let m = bake_tile_mesh(&dem, Square { center: [0.0, 0.0], half: 50.0 }, 5, 100.0);
        // height == world x on this ramp.
        for p in &m.positions {
            assert!((p[1] - p[0]).abs() < 1e-2, "pos.y {} != world x {}", p[1], p[0]);
        }
    }
}
