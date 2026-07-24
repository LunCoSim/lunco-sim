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
    /// Normal of the parent lattice, paired with `morph_targets`. Lerped
    /// alongside the position so shading tracks the surface actually drawn —
    /// see `ATTRIBUTE_MORPH_NORMAL`.
    pub morph_normals: Vec<[f32; 3]>,
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
/// `morph_src` is the surface the CDLOD **morph targets** sample — band-limited
/// for the PARENT lattice's spacing (2× this tile's). The morph end-state is
/// what the parent tile actually renders; sampling it from this tile's finer
/// surface aliased rim-scale features across the 2×-spaced even lattice (the
/// mid-field "sawtooth craters") and made the tile→parent swap pop. Pass the
/// same source twice when no distinct parent gate exists (tests, flat ground).
pub fn bake_tile_mesh(
    src: &dyn HeightSource,
    morph_src: &dyn HeightSource,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
    // Surface height at the tile centre, subtracted from every vertex Y so the mesh is
    // **local to its tile's `CellCoord`** in Y as well as X/Z (see `origin_xz`). Without
    // it, DEM tiles baked absolute Y (~+1945 m on the Moon) while the tile entity anchors
    // at that height — putting geometry ~2 km from its own origin, one big_space cell off
    // the content, which broke LOD/culling/colliders. Pass the same value used to place
    // the tile (`spawn_tile`/collider ring). `0.0` = DEM-absolute Y (flat scenes).
    origin_y: f64,
) -> TileMesh {
    let res = res.max(2);
    let n = res as f64;
    let step = region.side() / (n - 1.0);
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let inv_uv = 1.0 / (2.0 * dem_half_extent);
    let (ox, oz) = (origin_xz[0], origin_xz[1]);

    let world =
        |ix: usize, iz: usize| -> (f64, f64) { (x0 + ix as f64 * step, z0 + iz as f64 * step) };
    let height = |wx: f64, wz: f64| -> f32 { (src.height_at(wx, wz) - origin_y) as f32 };

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
    //
    // A sub-metre eps is only meaningful because the base DEM is interpolated
    // C1 (Catmull-Rom, `HeightGrid::height_at`). While it was bilinear the
    // gradient was constant within each ~4 m cell, so every probe in that cell
    // returned an identical normal and the terrain shaded as per-cell facets.
    // Widening eps to the cell size hides that, but band-limits the normals to
    // the DEM posting and erases the sub-cell relief the over-zoom layer adds —
    // so the interpolant, not the probe, is where that has to be fixed. Do not
    // "fix" faceting here by growing eps.
    let eps = 0.5;
    let normal_at = |wx: f64, wz: f64| -> [f32; 3] {
        let n = src.normal_at(wx, wz, eps);
        [n[0] as f32, n[1] as f32, n[2] as f32]
    };

    // The normal of the PARENT surface — the one that belongs to the morph target.
    // Sampled from `morph_src` (band-limited to the parent's Nyquist), so it
    // describes the coarse lattice the tile collapses onto rather than the fine
    // detail that lattice cannot represent.
    let morph_normal_at = |wx: f64, wz: f64| -> [f32; 3] {
        let n = morph_src.normal_at(wx, wz, eps);
        [n[0] as f32, n[1] as f32, n[2] as f32]
    };

    // Pre-sample the PARENT-gated surface on the even lattice — the heights every
    // morph target lerps toward, and the normals that go with them. One sample per
    // even/even vertex (res²/4), each shared by up to four vertices that snap to it.
    let even = res.div_ceil(2);
    let mut parent_y = vec![0.0f32; even * even];
    let mut parent_n = vec![[0.0f32, 1.0, 0.0]; even * even];
    for iz in (0..res).step_by(2) {
        for ix in (0..res).step_by(2) {
            let (wx, wz) = world(ix, iz);
            let k = (iz / 2) * even + (ix / 2);
            parent_y[k] = (morph_src.height_at(wx, wz) - origin_y) as f32;
            parent_n[k] = morph_normal_at(wx, wz);
        }
    }

    let mut positions = Vec::with_capacity(res * res);
    let mut morph_targets = Vec::with_capacity(res * res);
    let mut morph_normals = Vec::with_capacity(res * res);
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

            // Snap to the parent's even lattice, with the PARENT-gated height —
            // NOT this tile's own finer height at that spot. Even/even vertices
            // morph in place vertically onto the parent's value too, so the fully
            // morphed tile IS the parent surface (pop-free swap, no aliasing).
            let (sx, sz) = world(ix & !1, iz & !1);
            let k = (iz / 2) * even + (ix / 2);
            let sy = parent_y[k];
            morph_targets.push([(sx - ox) as f32, sy, (sz - oz) as f32]);
            // The normal at that same snapped point, so shading follows the
            // geometry through the whole morph instead of lagging on the fine
            // surface the tile is morphing AWAY from.
            morph_normals.push(parent_n[k]);

            normals.push(normal_at(wx, wz));
            uvs.push([
                ((wx + dem_half_extent) * inv_uv) as f32,
                ((wz + dem_half_extent) * inv_uv) as f32,
            ]);
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
        &mut morph_normals,
        &mut normals,
        &mut uvs,
        &mut indices,
    );

    TileMesh {
        positions,
        morph_targets,
        morph_normals,
        normals,
        uvs,
        indices,
    }
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
    morph_normals: &mut Vec<[f32; 3]>,
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
            let mn = morph_normals[gi];
            let uv = uvs[gi];
            skirt.push(positions.len() as u32);
            positions.push([p[0], p[1] - skirt_depth, p[2]]);
            morph_targets.push([mt[0], mt[1] - skirt_depth, mt[2]]);
            morph_normals.push(mn); // hidden wall — reuse the edge morph normal
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
        HeightGrid {
            res: 8,
            half_extent: 100.0,
            heights: vec![0.0; 64],
        }
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
        HeightGrid {
            res,
            half_extent: half,
            heights,
        }
    }

    #[test]
    fn flat_dem_bakes_flat_no_morph() {
        let res = 5;
        let dem = flat_dem();
        let m = bake_tile_mesh(
            &dem,
            &dem,
            Square {
                center: [0.0, 0.0],
                half: 50.0,
            },
            res,
            100.0,
            [0.0, 0.0],
            0.0,
        );
        // Interior grid first, then appended skirt verts.
        assert!(m.positions.len() >= res * res);
        assert!(m.indices.len() >= 4 * 4 * 6);
        // Interior surface is flat at y=0; skirt verts hang below it.
        assert!(m.positions[..res * res].iter().all(|p| p[1] == 0.0));
        assert!(m.positions[res * res..].iter().all(|p| p[1] < 0.0));
        // Flat → up normals (interior + skirts copy the edge normal).
        assert!(m.normals.iter().all(|n| n[1] > 0.99));
    }

    /// DIAGNOSTIC for "newly appeared LODs are black / wrongly lit".
    ///
    /// A tile spawns at `reveal = 0`, which the vertex shader turns into `m = 1`:
    /// every vertex is drawn AT ITS MORPH TARGET, i.e. the surface on screen is the
    /// coarse parent lattice. But the mesh carries only ONE normal set, sampled at
    /// the tile's own FINE position, and the shader never morphs it. So for the
    /// whole reveal the geometry is the parent surface while the shading is the
    /// child surface.
    ///
    /// Quantifies the disagreement and — with a grazing lunar sun — how many
    /// vertices have `N·L` of the WRONG SIGN, which is what reads as black.
    #[test]
    fn morphed_geometry_is_shaded_with_unmorphed_normals() {
        // Bumpy DEM: fine relief the parent lattice cannot represent.
        let res_dem = 33;
        let half = 100.0f32;
        let s = (2.0 * half) / (res_dem as f32 - 1.0);
        let mut heights = vec![0.0f64; res_dem * res_dem];
        for z in 0..res_dem {
            for x in 0..res_dem {
                let wx = (-half + x as f32 * s) as f64;
                let wz = (-half + z as f32 * s) as f64;
                heights[z * res_dem + x] = 6.0 * (wx * 0.09).sin() * (wz * 0.09).cos();
            }
        }
        let dem = HeightGrid {
            res: res_dem,
            half_extent: half,
            heights,
        };

        let region = Square {
            center: [0.0, 0.0],
            half: 50.0,
        };
        let res = 17;
        let m = bake_tile_mesh(&dem, &dem, region, res, half as f64, [0.0, 0.0], 0.0);

        // Sun 12 deg above the horizon — the grazing case the lunar BRDF is built
        // for, and the one that makes a normal error flip the lit/unlit decision.
        let el: f32 = 12f32.to_radians();
        let l = [el.cos(), el.sin(), 0.0f32];

        let dot = |n: &[f32; 3]| n[0] * l[0] + n[1] * l[1] + n[2] * l[2];

        // True normal of the surface ACTUALLY DRAWN at m=1 (the parent lattice),
        // approximated per-triangle from the morph targets.
        let mut flipped = 0usize;
        let mut worst_deg = 0.0f32;
        let n_grid = res * res;
        for iz in 0..res - 1 {
            for ix in 0..res - 1 {
                let i = iz * res + ix;
                if i + res + 1 >= n_grid {
                    continue;
                }
                let p = |k: usize| m.morph_targets[k];
                let (a, b, c) = (p(i), p(i + 1), p(i + res));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let mut fnorm = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = (fnorm[0] * fnorm[0] + fnorm[1] * fnorm[1] + fnorm[2] * fnorm[2]).sqrt();
                if len < 1e-6 {
                    continue; // degenerate (collapsed morph quad)
                }
                for e in fnorm.iter_mut() {
                    *e /= len;
                }
                if fnorm[1] < 0.0 {
                    for e in fnorm.iter_mut() {
                        *e = -*e;
                    }
                }
                let shaded = m.normals[i]; // what the shader actually uses
                let d_true = dot(&fnorm);
                let d_shaded = dot(&shaded);
                if (d_true > 0.0) != (d_shaded > 0.0) {
                    flipped += 1;
                }
                let cosang = (fnorm[0] * shaded[0] + fnorm[1] * shaded[1] + fnorm[2] * shaded[2])
                    .clamp(-1.0, 1.0);
                worst_deg = worst_deg.max(cosang.acos().to_degrees());
            }
        }
        let quads = (res - 1) * (res - 1);
        println!(
            "UNMORPHED normal vs drawn geometry: worst {worst_deg:.1} deg, \
             {flipped}/{quads} quads flip the sign of N.L (would shade black)"
        );
        assert!(
            worst_deg > 5.0,
            "expected a real mismatch to reproduce the artifact; got {worst_deg:.2} deg"
        );

        // THE FIX: `morph_normals` is the normal of the parent lattice, and the
        // vertex shader lerps normal by the same factor as position. At m = 1 the
        // shaded normal IS `morph_normals`, so it must agree with the geometry
        // being drawn far better than the fine normal did.
        let mut worst_fixed = 0.0f32;
        let mut flipped_fixed = 0usize;
        for iz in 0..res - 1 {
            for ix in 0..res - 1 {
                let i = iz * res + ix;
                if i + res + 1 >= n_grid {
                    continue;
                }
                let p = |k: usize| m.morph_targets[k];
                let (a, b, c) = (p(i), p(i + 1), p(i + res));
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let mut fnorm = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                let len = (fnorm[0] * fnorm[0] + fnorm[1] * fnorm[1] + fnorm[2] * fnorm[2]).sqrt();
                if len < 1e-6 {
                    continue;
                }
                for e in fnorm.iter_mut() {
                    *e /= len;
                }
                if fnorm[1] < 0.0 {
                    for e in fnorm.iter_mut() {
                        *e = -*e;
                    }
                }
                let shaded = m.morph_normals[i];
                if (dot(&fnorm) > 0.0) != (dot(&shaded) > 0.0) {
                    flipped_fixed += 1;
                }
                let cosang = (fnorm[0] * shaded[0] + fnorm[1] * shaded[1] + fnorm[2] * shaded[2])
                    .clamp(-1.0, 1.0);
                worst_fixed = worst_fixed.max(cosang.acos().to_degrees());
            }
        }
        println!(
            "MORPHED normal vs drawn geometry:   worst {worst_fixed:.1} deg, \
             {flipped_fixed}/{quads} quads flip"
        );

        // THE CONTRACT. A fully-morphed child must look like its PARENT — the tile
        // that would be drawn instead of it. The parent shades from its own
        // band-limited surface at its own vertices, so `morph_normals` must be
        // exactly that: `morph_src`'s normal at the SNAPPED position. Checking
        // against the parent's analytic normal (not the morph grid's faceted
        // triangle normals, which are a piecewise-constant approximation of it and
        // differ for both the old and new attribute alike).
        let step = region.side() / (res as f64 - 1.0);
        let x0 = region.center[0] - region.half;
        let z0 = region.center[1] - region.half;
        let mut worst_contract = 0.0f32;
        for iz in 0..res {
            for ix in 0..res {
                let sx = x0 + (ix & !1) as f64 * step;
                let sz = z0 + (iz & !1) as f64 * step;
                let want = dem.normal_at(sx, sz, 0.5);
                let got = m.morph_normals[iz * res + ix];
                let cosang =
                    (want[0] as f32 * got[0] + want[1] as f32 * got[1] + want[2] as f32 * got[2])
                        .clamp(-1.0, 1.0);
                worst_contract = worst_contract.max(cosang.acos().to_degrees());
            }
        }
        assert!(
            worst_contract < 0.5,
            "morph normal must be the PARENT surface normal at the snapped point \
             (off by up to {worst_contract:.2} deg) — otherwise a fully-morphed \
             tile shades unlike the parent it stands in for"
        );

        // And it must actually carry NEW information: if it merely duplicated the
        // fine normals the whole attribute would be a no-op and tiles would still
        // spawn shaded for geometry they do not have.
        let differing = (0..res * res)
            .filter(|&i| {
                let (a, b) = (m.normals[i], m.morph_normals[i]);
                let c = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
                c.acos().to_degrees() > 1.0
            })
            .count();
        assert!(
            differing * 4 > res * res,
            "only {differing}/{} morph normals differ from the fine normals — the \
             attribute is not carrying the parent surface",
            res * res
        );
    }

    /// DIAGNOSTIC for "terrain still flashes black while moving".
    ///
    /// Bevy derives a mesh's `Aabb` from `ATTRIBUTE_POSITION` alone. The geomorph
    /// vertex shader DISPLACES every vertex toward its morph target, and a tile
    /// spawns fully morphed (`reveal = 0`), so the geometry actually rasterised can
    /// sit outside the box culling is tested against. When it does, a tile whose
    /// box has left the frustum is culled while its drawn surface is still on
    /// screen — a hole, which renders as the clear colour: a black flash at the
    /// screen edge while the camera moves.
    ///
    /// Measures how far outside the position-derived box the morph targets reach.
    #[test]
    fn morph_targets_escape_the_position_derived_bounds() {
        let res_dem = 33;
        let half = 100.0f32;
        let s = (2.0 * half) / (res_dem as f32 - 1.0);
        let mut heights = vec![0.0f64; res_dem * res_dem];
        for z in 0..res_dem {
            for x in 0..res_dem {
                let wx = (-half + x as f32 * s) as f64;
                let wz = (-half + z as f32 * s) as f64;
                heights[z * res_dem + x] = 6.0 * (wx * 0.09).sin() * (wz * 0.09).cos();
            }
        }
        let dem = HeightGrid {
            res: res_dem,
            half_extent: half,
            heights,
        };
        let m = bake_tile_mesh(
            &dem,
            &dem,
            Square {
                center: [0.0, 0.0],
                half: 50.0,
            },
            17,
            half as f64,
            [0.0, 0.0],
            0.0,
        );

        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &m.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let mut worst = 0.0f32;
        for t in &m.morph_targets {
            for k in 0..3 {
                worst = worst.max(lo[k] - t[k]).max(t[k] - hi[k]);
            }
        }
        println!(
            "morph targets exceed the POSITION bounds by up to {worst:.3} m \
             (bounds y: {:.2}..{:.2})",
            lo[1], hi[1]
        );
        // Measured at 0 m: the skirts extend the box well below the surface and the
        // parent is a SMOOTHED version of the child, so the displaced geometry stays
        // inside the box culling tests. Pinned because if it ever stops holding,
        // tiles get culled while still visible and the holes read as black.
        assert!(
            worst <= 0.0,
            "morph targets reach {worst:.3} m outside the position-derived Aabb — \
             displaced geometry can now be frustum-culled while on screen; the tile \
             needs an explicit Aabb covering both position and morph target"
        );
    }

    #[test]
    fn even_vertices_do_not_move_odd_collapse_to_even() {
        let dem = ramp_dem();
        let region = Square {
            center: [0.0, 0.0],
            half: 50.0,
        };
        let res = 5;
        let m = bake_tile_mesh(&dem, &dem, region, res, 100.0, [0.0, 0.0], 0.0);
        let step = region.side() / (res as f64 - 1.0);
        let x0 = region.center[0] - region.half;
        for iz in 0..res {
            for ix in 0..res {
                let i = iz * res + ix;
                if ix % 2 == 0 {
                    // Even vertex: morph target X == own X (no lateral move).
                    assert!(
                        (m.morph_targets[i][0] - m.positions[i][0]).abs() < 1e-3,
                        "even vtx moved"
                    );
                } else {
                    // Odd vertex: collapses to the lower even neighbour's X.
                    let snapped_x = (x0 + (ix & !1) as f64 * step) as f32;
                    assert!((m.morph_targets[i][0] - snapped_x).abs() < 1e-3);
                    // On an X-ramp, morphed height = snapped world X.
                    assert!(
                        (m.morph_targets[i][1] - snapped_x).abs() < 1e-2,
                        "morph height wrong"
                    );
                }
            }
        }
    }

    #[test]
    fn positions_carry_dem_height() {
        let dem = ramp_dem();
        let res = 5;
        let m = bake_tile_mesh(
            &dem,
            &dem,
            Square {
                center: [0.0, 0.0],
                half: 50.0,
            },
            res,
            100.0,
            [0.0, 0.0],
            0.0,
        );
        // height == world x on this ramp (interior verts only; skirts hang below).
        for p in &m.positions[..res * res] {
            assert!(
                (p[1] - p[0]).abs() < 1e-2,
                "pos.y {} != world x {}",
                p[1],
                p[0]
            );
        }
    }
}
