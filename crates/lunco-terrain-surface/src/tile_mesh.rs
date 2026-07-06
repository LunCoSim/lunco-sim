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

use lunco_obstacle_field::field::grid_indices;
use lunco_terrain_core::HeightSource;

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
/// the composed height `src` (the terrain's `SurfaceOracle`: DEM base + analytic
/// crater/edit modifiers — so rims resolve at *this tile's* vertex density, not the
/// DEM grid's). `res` is clamped ≥ 2. `MORPH_TARGET` snaps each vertex index down
/// to the parent's even lattice (`idx & !1`) and re-samples the source there — so
/// even vertices don't move and odd vertices collapse onto their even neighbour,
/// the standard CDLOD vertex morph. UVs are DEM-global (`(world + H)/(2H)`) so
/// layer maps align across tiles. `dem_half_extent` is the DEM's `half_extent`.
///
/// `origin_xz` is subtracted from vertex X/Z so positions are **relative to that
/// anchor** (UVs stay DEM-global). Pass the tile's own world centre to keep
/// vertices small and f32-precise when the tile is anchored to its own big_space
/// `CellCoord`; pass `[0.0, 0.0]` for DEM-absolute positions.
pub fn bake_tile_mesh(
    src: &dyn HeightSource,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
) -> TileMesh {
    let res = res.max(2);
    let n = res as f64;
    let step = region.side() / (n - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let inv_uv = 1.0 / (2.0 * dem_half_extent);
    let (ox, oz) = (origin_xz[0], origin_xz[1]);

    let world = |ix: usize, iz: usize| -> (f64, f64) {
        (x0 + ix as f64 * step, z0 + iz as f64 * step)
    };
    let height = |wx: f64, wz: f64| -> f32 { src.height_at(wx, wz) as f32 };

    // Normals are sampled ANALYTICALLY from the composed source, NOT from each
    // tile's own grid — per-tile finite-difference normals don't agree at shared
    // edges (visible shading "stitching"); the analytic field removes that.
    //
    // The central-difference `eps` is a FIXED world scale, identical at every
    // LOD depth. It used to scale with the tile's vertex spacing ("proper
    // normal LOD") — but the lunar BRDF (`regolith_factor`) is normal-driven,
    // so per-depth eps meant per-depth *brightness*: crater tiles (refined to
    // max depth by the error metric) shaded visibly differently from the
    // surrounding coarse flat tiles, and every LOD boundary stepped in tone
    // (the tile-sized "checkerboard" patches). The surface an eps probes is
    // already band-limited per tile (`detail_limited(step)` in the bake), so
    // far tiles keep their smoothing through the SURFACE, not the probe — a
    // fixed feature-scale probe keeps tone continuous across depths.
    let eps = 0.5;
    let normal_at = |wx: f64, wz: f64| -> [f32; 3] {
        let n = src.normal_at(wx, wz, eps);
        [n[0] as f32, n[1] as f32, n[2] as f32]
    };

    let mut positions = Vec::with_capacity(res * res);
    let mut morph_targets = Vec::with_capacity(res * res);
    let mut normals = Vec::with_capacity(res * res);
    let mut uvs = Vec::with_capacity(res * res);
    let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
    for iz in 0..res {
        for ix in 0..res {
            let (wx, wz) = world(ix, iz);
            let y = height(wx, wz);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            positions.push([(wx - ox) as f32, y, (wz - oz) as f32]);

            // Snap to the parent's even lattice and re-sample.
            let (sx, sz) = world(ix & !1, iz & !1);
            morph_targets.push([(sx - ox) as f32, height(sx, sz), (sz - oz) as f32]);

            normals.push(normal_at(wx, wz));
            uvs.push([((wx + dem_half_extent) * inv_uv) as f32, ((wz + dem_half_extent) * inv_uv) as f32]);
        }
    }

    let mut indices = grid_indices(res);

    // --- Skirts: hide T-junction cracks between neighbouring tiles of different
    // LOD. The per-tile morph band can't guarantee a finer tile's edge matches its
    // coarser neighbour's straight edge, so a thin vertical wall is dropped around
    // the perimeter to cover the gap. Depth scales with the tile's relief (+ a
    // small floor) so it always spans the gap without a needlessly tall wall.
    let skirt_depth = ((max_y - min_y) * 0.75 + region.side() as f32 * 0.05).max(0.5);
    append_skirts(
        res,
        skirt_depth,
        &mut positions,
        &mut morph_targets,
        &mut normals,
        &mut uvs,
        &mut indices,
    );

    TileMesh { positions, morph_targets, normals, uvs, indices }
}

/// Append a downward skirt wall around the tile perimeter. For each of the four
/// edges, every edge vertex gets a duplicate dropped by `skirt_depth`, and each
/// segment becomes a double-sided wall quad (both windings → never culled, so the
/// crack is covered from any angle). Skirt verts carry the edge vertex's morph
/// target (also dropped) so the wall follows the surface as it geomorphs.
#[allow(clippy::too_many_arguments)]
fn append_skirts(
    res: usize,
    skirt_depth: f32,
    positions: &mut Vec<[f32; 3]>,
    morph_targets: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<u32>,
) {
    // The four perimeter runs as ordered grid-vertex indices.
    let top: Vec<usize> = (0..res).collect();
    let bottom: Vec<usize> = (0..res).map(|ix| (res - 1) * res + ix).collect();
    let left: Vec<usize> = (0..res).map(|iz| iz * res).collect();
    let right: Vec<usize> = (0..res).map(|iz| iz * res + (res - 1)).collect();

    for edge in [top, bottom, left, right] {
        // Drop a skirt vertex below each edge vertex, recording its new index.
        let mut skirt: Vec<u32> = Vec::with_capacity(edge.len());
        for &gi in &edge {
            let p = positions[gi];
            let mt = morph_targets[gi];
            let n = normals[gi];
            let uv = uvs[gi];
            skirt.push(positions.len() as u32);
            positions.push([p[0], p[1] - skirt_depth, p[2]]);
            morph_targets.push([mt[0], mt[1] - skirt_depth, mt[2]]);
            normals.push(n); // hidden wall — reuse the edge normal
            uvs.push(uv);
        }
        // Wall quads per segment, emitted with BOTH windings (double-sided).
        for k in 0..edge.len() - 1 {
            let (a, b) = (edge[k] as u32, edge[k + 1] as u32);
            let (sa, sb) = (skirt[k], skirt[k + 1]);
            // front winding
            indices.extend_from_slice(&[a, sa, b, b, sa, sb]);
            // back winding
            indices.extend_from_slice(&[a, b, sa, b, sb, sa]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_obstacle_field::field::HeightGrid;

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
        let res = 5;
        let m = bake_tile_mesh(&flat_dem(), Square { center: [0.0, 0.0], half: 50.0 }, res, 100.0, [0.0, 0.0]);
        // Interior grid first, then appended skirt verts.
        assert!(m.positions.len() >= res * res);
        assert!(m.indices.len() >= 4 * 4 * 6);
        // Interior surface is flat at y=0; skirt verts hang below it.
        assert!(m.positions[..res * res].iter().all(|p| p[1] == 0.0));
        assert!(m.positions[res * res..].iter().all(|p| p[1] < 0.0));
        // Flat → up normals (interior + skirts copy the edge normal).
        assert!(m.normals.iter().all(|n| n[1] > 0.99));
    }

    #[test]
    fn even_vertices_do_not_move_odd_collapse_to_even() {
        let dem = ramp_dem();
        let region = Square { center: [0.0, 0.0], half: 50.0 };
        let res = 5;
        let m = bake_tile_mesh(&dem, region, res, 100.0, [0.0, 0.0]);
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
        let res = 5;
        let m = bake_tile_mesh(&dem, Square { center: [0.0, 0.0], half: 50.0 }, res, 100.0, [0.0, 0.0]);
        // height == world x on this ramp (interior verts only; skirts hang below).
        for p in &m.positions[..res * res] {
            assert!((p[1] - p[0]).abs() < 1e-2, "pos.y {} != world x {}", p[1], p[0]);
        }
    }
}
