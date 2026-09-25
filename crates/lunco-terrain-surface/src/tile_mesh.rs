//! Per-tile CDLOD mesh bake (milestone S2a, approach C).
//!
//! Each quadtree node bakes a real grid mesh from the DEM. Every vertex carries
//! two positions: its **own** LOD position (`POSITION`, `y = DEM height`) and a
//! **`MORPH_TARGET`** — the same vertex evaluated on the parent node's coarser
//! lattice by bilinear interpolation. The CDLOD vertex shader lerps
//! `pos = mix(POSITION, MORPH_TARGET, morph)` by a camera-distance morph factor,
//! so a tile geomorphs smoothly into its parent with no popping and no
//! texture fetch. The physics collider ring is deliberately a separate product:
//! it samples the same oracle through its authored physics lattice and never
//! consumes visual tile resolution or render assets. Static colliders use the
//! native materialized product. This module therefore owns visual tile geometry
//! only (see `docs/architecture/terrain-substrate.md`).
//!
//! Pure + Bevy-free → unit-tested and wasm-safe; the plugin runs it off-thread
//! and assembles the attributes into a Bevy `Mesh`.

use lunco_obstacle_field::field::grid_indices;
use lunco_terrain_core::{
    HeightSource, normal_at_bounded, square_boundary_height_at,
    square_boundary_posting_spacing, square_boundary_sample_coordinate,
};

use lunco_terrain_core::quadtree::Square;

/// CPU vertex data for one CDLOD tile. `morph_targets[i]` is the position on
/// the parent-lattice surface at vertex `i` as the camera recedes. Positions are in
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
    /// Perimeter membership in `[top, bottom, left, right]` order.  The
    /// streamer uses this to stitch only edges whose neighbour is coarser.
    pub edge_masks: Vec<[f32; 4]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Bake a `res × res`-vertex CDLOD mesh covering `region`, sampling heights from
/// the composed height `src` (the terrain's `SurfaceOracle`: DEM base + analytic
/// crater/edit modifiers — so rims resolve at *this tile's* vertex density, not the
/// DEM grid's). `res` is clamped ≥ 2. `MORPH_TARGET` evaluates the parent
/// lattice at each vertex's exact X/Z position — so even vertices coincide and
/// odd vertices use the parent's linear interpolation rather than a stepped
/// duplicate. UVs are DEM-global (`(world + H)/(2H)`) so
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
pub fn bake_tile_mesh<S: HeightSource, M: HeightSource>(
    // Generic, not `&dyn`: the bake samples the source once per lattice point
    // (plus the morph lattice), and per-sample virtual dispatch blocked
    // inlining of the whole composed-oracle chain. Callers pass the concrete
    // band-limited `SurfaceOracle` pair; values are identical either way.
    src: &S,
    morph_src: &M,
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

    // Fine surface: ONE padded `(res+2)²` height lattice — the vertex grid plus a
    // one-step ghost ring — sampled once, then positions AND central-difference
    // normals are read from it. The previous shape (height + a 4-tap analytic
    // probe per vertex) was 5 oracle evals per vertex; this is ~1.
    //
    // Two hard-won properties this must keep:
    // - **No shading "stitching" at shared edges.** Per-tile FD normals off each
    //   tile's OWN interior grid disagreed at tile seams. The ghost ring fixes
    //   that a different way than the old analytic probe did: same-depth
    //   neighbours sample the same band-limited source at the same world lattice
    //   points (the ghost ring IS the neighbour's first column), so edge normals
    //   are identical by construction.
    // - **Tone continuity across LOD depths.** The lunar BRDF is normal-driven,
    //   and a per-depth probe scale once meant per-depth brightness (tile-sized
    //   "checkerboard"). The stencil here DOES ride the tile's step — but the
    //   surface it probes is band-limited per tile (`detail_limited(step)` in
    //   the bake), so each depth's normals describe that depth's own surface,
    //   exactly like the morph normals below. If tone stepping ever reappears,
    //   suspect the band gate, not the stencil width.
    //
    // Sub-vertex relief still needs the base DEM interpolated C1 (Catmull-Rom,
    // `HeightGrid::height_at`) — while it was bilinear, the gradient was
    // constant per DEM cell and the terrain shaded as facets. The interpolant,
    // not this stencil, is where that lives.
    let pad = res + 2;
    let mut lattice = vec![0.0f64; pad * pad];
    for iz in 0..pad {
        let wz = z0 + (iz as f64 - 1.0) * step;
        for ix in 0..pad {
            let wx = x0 + (ix as f64 - 1.0) * step;
            lattice[iz * pad + ix] = src.height_at(wx, wz);
        }
    }
    // Lattice lookup in VERTEX indices (ghost ring at -1 and `res`).
    let h_at =
        |ix: isize, iz: isize| -> f64 { lattice[(iz + 1) as usize * pad + (ix + 1) as usize] };
    let fine_normal = |ix: isize, iz: isize| -> [f32; 3] {
        let hx = h_at(ix + 1, iz) - h_at(ix - 1, iz);
        let hz = h_at(ix, iz + 1) - h_at(ix, iz - 1);
        // gradient of height field → normal (−dY/dx, 1, −dY/dz), normalised —
        // the same formula as `HeightSource::normal_at` with `eps = step`.
        let nx = -hx / (2.0 * step);
        let nz = -hz / (2.0 * step);
        let len = (nx * nx + 1.0 + nz * nz).sqrt();
        [(nx / len) as f32, (1.0 / len) as f32, (nz / len) as f32]
    };

    // The PARENT lattice: ONE padded `(even+2)²` sample of the parent-gated
    // surface on the parent's own 2×-spaced grid — the heights every morph
    // target lerps toward, central-differenced for the normals that go with
    // them. Exactly the shape of the fine lattice above, one step coarser.
    //
    // This is also what the PARENT TILE ITSELF computes: its `fine_normal` is a
    // central difference on this very lattice, at this very spacing, off this
    // very band. The morph contract — a fully-morphed child must shade like the
    // parent that replaces it — is therefore satisfied by construction rather
    // than approximated. (It used to be a 4-tap analytic probe at a fixed 0.5 m
    // per even vertex: `res²` extra oracle evaluations, as many as the entire
    // fine lattice, to approximate a value the parent derives differently.)
    let even = res.div_ceil(2);
    let pstep = 2.0 * step;
    let ppad = even + 2;
    let mut plattice = vec![0.0f64; ppad * ppad];
    for iz in 0..ppad {
        let wz = z0 + (iz as f64 - 1.0) * pstep;
        for ix in 0..ppad {
            let wx = x0 + (ix as f64 - 1.0) * pstep;
            plattice[iz * ppad + ix] = morph_src.height_at(wx, wz);
        }
    }
    let ph_at =
        |ix: isize, iz: isize| -> f64 { plattice[(iz + 1) as usize * ppad + (ix + 1) as usize] };
    let mut parent_y = vec![0.0f32; even * even];
    let mut parent_n = vec![[0.0f32, 1.0, 0.0]; even * even];
    for ez in 0..even {
        for ex in 0..even {
            let k = ez * even + ex;
            parent_y[k] = (ph_at(ex as isize, ez as isize) - origin_y) as f32;
            let (pwx, pwz) = world((ex * 2).min(res - 1), (ez * 2).min(res - 1));
            let edge =
                pwx.abs() >= dem_half_extent - 1.0e-9 || pwz.abs() >= dem_half_extent - 1.0e-9;
            parent_n[k] = if edge {
                let n = normal_at_bounded(morph_src, pwx, pwz, pstep, dem_half_extent);
                [n[0] as f32, n[1] as f32, n[2] as f32]
            } else {
                let hx = ph_at(ex as isize + 1, ez as isize) - ph_at(ex as isize - 1, ez as isize);
                let hz = ph_at(ex as isize, ez as isize + 1) - ph_at(ex as isize, ez as isize - 1);
                let nx = -hx / (2.0 * pstep);
                let nz = -hz / (2.0 * pstep);
                let len = (nx * nx + 1.0 + nz * nz).sqrt();
                [(nx / len) as f32, (1.0 / len) as f32, (nz / len) as f32]
            };
        }
    }

    let mut positions = Vec::with_capacity(res * res);
    let mut morph_targets = Vec::with_capacity(res * res);
    let mut morph_normals = Vec::with_capacity(res * res);
    let mut normals = Vec::with_capacity(res * res);
    let mut edge_masks = Vec::with_capacity(res * res);
    let mut uvs = Vec::with_capacity(res * res);
    for iz in 0..res {
        for ix in 0..res {
            let (wx, wz) = world(ix, iz);
            let y = (h_at(ix as isize, iz as isize) - origin_y) as f32;
            positions.push([(wx - ox) as f32, y, (wz - oz) as f32]);

            // Evaluate the PARENT-gated surface at this vertex's exact position.
            // The parent tile is a regular grid whose triangles linearly
            // interpolate between its lattice samples. Repeating the lower even
            // sample for an odd child vertex (the old implementation) created a
            // stepped edge and therefore a T-junction even when explicitly
            // stitched.
            let ex = ix / 2;
            let ez = iz / 2;
            let ex1 = (ex + 1).min(even - 1);
            let ez1 = (ez + 1).min(even - 1);
            let tx = if ex1 == ex {
                0.0
            } else {
                (ix & 1) as f32 * 0.5
            };
            let tz = if ez1 == ez {
                0.0
            } else {
                (iz & 1) as f32 * 0.5
            };
            let sy = {
                let a = parent_y[ez * even + ex];
                let b = parent_y[ez * even + ex1];
                let c = parent_y[ez1 * even + ex];
                let d = parent_y[ez1 * even + ex1];
                let ab = a + (b - a) * tx;
                let cd = c + (d - c) * tx;
                ab + (cd - ab) * tz
            };
            let pn = {
                let a = parent_n[ez * even + ex];
                let b = parent_n[ez * even + ex1];
                let c = parent_n[ez1 * even + ex];
                let d = parent_n[ez1 * even + ex1];
                let ab = [
                    a[0] + (b[0] - a[0]) * tx,
                    a[1] + (b[1] - a[1]) * tx,
                    a[2] + (b[2] - a[2]) * tx,
                ];
                let cd = [
                    c[0] + (d[0] - c[0]) * tx,
                    c[1] + (d[1] - c[1]) * tx,
                    c[2] + (d[2] - c[2]) * tx,
                ];
                let p = [
                    ab[0] + (cd[0] - ab[0]) * tz,
                    ab[1] + (cd[1] - ab[1]) * tz,
                    ab[2] + (cd[2] - ab[2]) * tz,
                ];
                let len = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                [p[0] / len, p[1] / len, p[2] / len]
            };
            morph_targets.push([(wx - ox) as f32, sy, (wz - oz) as f32]);
            morph_normals.push(pn);

            let edge = wx.abs() >= dem_half_extent - 1.0e-9 || wz.abs() >= dem_half_extent - 1.0e-9;
            normals.push(if edge {
                // The padded lattice intentionally covers interior tile seams,
                // but its outer samples are outside the measured DEM. Use the
                // same bounded one-sided derivative as point queries there;
                // clamped ghost values otherwise halve the boundary slope.
                let n = normal_at_bounded(src, wx, wz, step, dem_half_extent);
                [n[0] as f32, n[1] as f32, n[2] as f32]
            } else {
                fine_normal(ix as isize, iz as isize)
            });
            edge_masks.push([
                (iz == 0) as u8 as f32,
                (iz == res - 1) as u8 as f32,
                (ix == 0) as u8 as f32,
                (ix == res - 1) as u8 as f32,
            ]);
            uvs.push([
                ((wx + dem_half_extent) * inv_uv) as f32,
                ((wz + dem_half_extent) * inv_uv) as f32,
            ]);
        }
    }

    // The grid is deliberately closed by edge stitching in the vertex stage.
    // The shared parent lattice is the exact seam surface. The renderer applies
    // edge stitching only where the draw partition has a coarser neighbour, so
    // this mesh stays a regular grid with no fabricated skirt geometry.
    let indices = grid_indices(res);

    TileMesh {
        positions,
        morph_targets,
        morph_normals,
        normals,
        edge_masks,
        uvs,
        indices,
    }
}

/// Bake a regular CDLOD tile and replace any DEM perimeter edge with a strip
/// that follows the shared globe/surface boundary posting curve. The regular
/// tile remains band-limited; only its outer edge uses the full source, and its
/// transition into the regular interior is triangulated here during the
/// existing asynchronous tile bake.
#[allow(clippy::too_many_arguments)]
pub fn bake_tile_mesh_with_boundary<S: HeightSource, M: HeightSource, B: HeightSource>(
    src: &S,
    morph_src: &M,
    boundary_src: &B,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_grid_resolution: usize,
) -> TileMesh {
    let mut mesh = bake_tile_mesh(
        src,
        morph_src,
        region,
        res,
        dem_half_extent,
        origin_xz,
        origin_y,
    );
    stitch_dem_boundary(
        &mut mesh,
        boundary_src,
        region,
        res.max(2),
        dem_half_extent,
        origin_xz,
        origin_y,
        boundary_grid_resolution,
    );
    mesh
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DemEdge {
    Top,
    Bottom,
    Left,
    Right,
}

fn stitch_dem_boundary<B: HeightSource>(
    mesh: &mut TileMesh,
    boundary_src: &B,
    region: Square,
    res: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_grid_resolution: usize,
) {
    if res < 3 || boundary_grid_resolution < 2 || !dem_half_extent.is_finite() {
        return;
    }
    let Some(boundary_normal_epsilon) =
        square_boundary_posting_spacing(dem_half_extent, boundary_grid_resolution)
    else {
        return;
    };

    let step = region.side() / (res - 1) as f64;
    let x0 = region.center[0] - region.half;
    let z0 = region.center[1] - region.half;
    let on_edge = |coordinate: f64| {
        (coordinate.abs() - dem_half_extent.abs()).abs()
            <= (dem_half_extent.abs() * 1.0e-9).max(1.0e-6)
    };
    let mut active = [false; 4]; // top, bottom, left, right
    active[0] = on_edge(z0);
    active[1] = on_edge(z0 + region.side());
    active[2] = on_edge(x0);
    active[3] = on_edge(x0 + region.side());
    if !active.into_iter().any(|edge| edge) {
        return;
    }

    // The coarse boundary vertices must lie on the same posting-linear source
    // curve as the globe's cutout, including vertices between DEM postings.
    for edge in [DemEdge::Top, DemEdge::Bottom, DemEdge::Left, DemEdge::Right] {
        if !edge_active(edge, active) {
            continue;
        }
        for along_index in 0..res {
            let vertex = edge_vertex(edge, along_index, res);
            let (wx, wz) = edge_world(edge, along_index, res, x0, z0, step);
            update_boundary_vertex(
                mesh,
                vertex,
                boundary_src,
                wx,
                wz,
                dem_half_extent,
                boundary_grid_resolution,
                origin_xz,
                origin_y,
                boundary_normal_epsilon,
            );
        }
    }

    let mut indices = Vec::with_capacity(mesh.indices.len() + boundary_grid_resolution * 16);
    for iz in 0..res - 1 {
        for ix in 0..res - 1 {
            let outer_cell = (active[0] && iz == 0)
                || (active[1] && iz == res - 2)
                || (active[2] && ix == 0)
                || (active[3] && ix == res - 2);
            if outer_cell {
                continue;
            }
            let i = (iz * res + ix) as u32;
            indices.extend_from_slice(&[
                i,
                i + res as u32,
                i + 1,
                i + 1,
                i + res as u32,
                i + res as u32 + 1,
            ]);
        }
    }

    for edge in [DemEdge::Top, DemEdge::Bottom, DemEdge::Left, DemEdge::Right] {
        if edge_active(edge, active) {
            stitch_edge_strips(
                mesh,
                &mut indices,
                boundary_src,
                edge,
                active,
                res,
                x0,
                z0,
                step,
                dem_half_extent,
                boundary_grid_resolution,
                origin_xz,
                origin_y,
                boundary_normal_epsilon,
            );
        }
    }
    for corner in [Corner::TopLeft, Corner::TopRight, Corner::BottomLeft, Corner::BottomRight] {
        if corner_active(corner, active) {
            stitch_corner(
                mesh,
                &mut indices,
                boundary_src,
                corner,
                res,
                x0,
                z0,
                step,
                dem_half_extent,
                boundary_grid_resolution,
                origin_xz,
                origin_y,
                boundary_normal_epsilon,
            );
        }
    }
    mesh.indices = indices;
}

fn edge_active(edge: DemEdge, active: [bool; 4]) -> bool {
    active[match edge {
        DemEdge::Top => 0,
        DemEdge::Bottom => 1,
        DemEdge::Left => 2,
        DemEdge::Right => 3,
    }]
}

fn edge_vertex(edge: DemEdge, along_index: usize, res: usize) -> u32 {
    let index = match edge {
        DemEdge::Top => along_index,
        DemEdge::Bottom => (res - 1) * res + along_index,
        DemEdge::Left => along_index * res,
        DemEdge::Right => along_index * res + res - 1,
    };
    index as u32
}

fn edge_inner_vertex(edge: DemEdge, along_index: usize, res: usize) -> u32 {
    let index = match edge {
        DemEdge::Top => res + along_index,
        DemEdge::Bottom => (res - 2) * res + along_index,
        DemEdge::Left => along_index * res + 1,
        DemEdge::Right => along_index * res + res - 2,
    };
    index as u32
}

fn edge_world(
    edge: DemEdge,
    along_index: usize,
    res: usize,
    x0: f64,
    z0: f64,
    step: f64,
) -> (f64, f64) {
    match edge {
        DemEdge::Top => (x0 + along_index as f64 * step, z0),
        DemEdge::Bottom => (x0 + along_index as f64 * step, z0 + (res - 1) as f64 * step),
        DemEdge::Left => (x0, z0 + along_index as f64 * step),
        DemEdge::Right => (x0 + (res - 1) as f64 * step, z0 + along_index as f64 * step),
    }
}

fn segment_postings<B: HeightSource>(
    mesh: &mut TileMesh,
    boundary_src: &B,
    edge: DemEdge,
    segment: usize,
    res: usize,
    x0: f64,
    z0: f64,
    step: f64,
    dem_half_extent: f64,
    boundary_grid_resolution: usize,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_normal_epsilon: f64,
) -> Vec<u32> {
    let (x_start, z_start) = edge_world(edge, segment, res, x0, z0, step);
    let (x_end, z_end) = edge_world(edge, segment + 1, res, x0, z0, step);
    let start = match edge {
        DemEdge::Top | DemEdge::Bottom => x_start,
        DemEdge::Left | DemEdge::Right => z_start,
    };
    let end = match edge {
        DemEdge::Top | DemEdge::Bottom => x_end,
        DemEdge::Left | DemEdge::Right => z_end,
    };
    let min = start.min(end);
    let max = start.max(end);
    let half_f32 = dem_half_extent as f32;
    let posting_step = (2.0_f32 * half_f32) / (boundary_grid_resolution as f32 - 1.0);
    let mut points = Vec::new();
    points.push(edge_vertex(edge, segment, res));
    if posting_step.is_finite() && posting_step > 0.0 {
        let q0 = ((min as f32 + half_f32) / posting_step)
            .clamp(0.0, boundary_grid_resolution as f32 - 1.0);
        let q1 = ((max as f32 + half_f32) / posting_step)
            .clamp(0.0, boundary_grid_resolution as f32 - 1.0);
        let first = q0.ceil() as usize;
        let last = (q1.floor() as usize).min(boundary_grid_resolution - 1);
        let tolerance = (dem_half_extent.abs() * 1.0e-9).max(1.0e-6);
        if first <= last {
            for posting in first..=last {
                let Some(along) = square_boundary_sample_coordinate(
                    posting,
                    boundary_grid_resolution,
                    dem_half_extent,
                ) else {
                    continue;
                };
                if along <= min + tolerance || along >= max - tolerance {
                    continue;
                }
                let (wx, wz) = match edge {
                    DemEdge::Top => (along, z_start),
                    DemEdge::Bottom => (along, z_start),
                    DemEdge::Left => (x_start, along),
                    DemEdge::Right => (x_start, along),
                };
                if let Some(vertex) = append_boundary_vertex(
                    mesh,
                    boundary_src,
                    wx,
                    wz,
                    edge,
                    boundary_grid_resolution,
                    dem_half_extent,
                    origin_xz,
                    origin_y,
                    boundary_normal_epsilon,
                ) {
                    points.push(vertex);
                }
            }
        }
    }
    points.push(edge_vertex(edge, segment + 1, res));
    points
}

fn append_boundary_vertex<B: HeightSource>(
    mesh: &mut TileMesh,
    boundary_src: &B,
    wx: f64,
    wz: f64,
    edge: DemEdge,
    boundary_grid_resolution: usize,
    dem_half_extent: f64,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_normal_epsilon: f64,
) -> Option<u32> {
    let height = square_boundary_height_at(
        boundary_src,
        wx,
        wz,
        dem_half_extent,
        boundary_grid_resolution,
    )?;
    let n = normal_at_bounded(
        boundary_src,
        wx,
        wz,
        boundary_normal_epsilon,
        dem_half_extent,
    );
    let normal = [n[0] as f32, n[1] as f32, n[2] as f32];
    let position = [
        (wx - origin_xz[0]) as f32,
        (height - origin_y) as f32,
        (wz - origin_xz[1]) as f32,
    ];
    let mut edge_mask = [0.0; 4];
    edge_mask[match edge {
        DemEdge::Top => 0,
        DemEdge::Bottom => 1,
        DemEdge::Left => 2,
        DemEdge::Right => 3,
    }] = 1.0;
    let index = mesh.positions.len() as u32;
    mesh.positions.push(position);
    mesh.morph_targets.push(position);
    mesh.morph_normals.push(normal);
    mesh.normals.push(normal);
    mesh.edge_masks.push(edge_mask);
    mesh.uvs.push([
        ((wx + dem_half_extent) / (2.0 * dem_half_extent)) as f32,
        ((wz + dem_half_extent) / (2.0 * dem_half_extent)) as f32,
    ]);
    Some(index)
}

fn update_boundary_vertex<B: HeightSource>(
    mesh: &mut TileMesh,
    vertex: u32,
    boundary_src: &B,
    wx: f64,
    wz: f64,
    dem_half_extent: f64,
    boundary_grid_resolution: usize,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_normal_epsilon: f64,
) {
    let Some(height) = square_boundary_height_at(
        boundary_src,
        wx,
        wz,
        dem_half_extent,
        boundary_grid_resolution,
    ) else {
        return;
    };
    let n = normal_at_bounded(
        boundary_src,
        wx,
        wz,
        boundary_normal_epsilon,
        dem_half_extent,
    );
    let normal = [n[0] as f32, n[1] as f32, n[2] as f32];
    let position = [
        (wx - origin_xz[0]) as f32,
        (height - origin_y) as f32,
        (wz - origin_xz[1]) as f32,
    ];
    let index = vertex as usize;
    mesh.positions[index] = position;
    mesh.morph_targets[index] = position;
    mesh.normals[index] = normal;
    mesh.morph_normals[index] = normal;
}

#[derive(Clone, Copy)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

fn corner_active(corner: Corner, active: [bool; 4]) -> bool {
    match corner {
        Corner::TopLeft => active[0] && active[2],
        Corner::TopRight => active[0] && active[3],
        Corner::BottomLeft => active[1] && active[2],
        Corner::BottomRight => active[1] && active[3],
    }
}

fn stitch_edge_strips<B: HeightSource>(
    mesh: &mut TileMesh,
    indices: &mut Vec<u32>,
    boundary_src: &B,
    edge: DemEdge,
    active: [bool; 4],
    res: usize,
    x0: f64,
    z0: f64,
    step: f64,
    dem_half_extent: f64,
    boundary_grid_resolution: usize,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_normal_epsilon: f64,
) {
    let skip_start = match edge {
        DemEdge::Top | DemEdge::Bottom => active[2],
        DemEdge::Left | DemEdge::Right => active[0],
    };
    let skip_end = match edge {
        DemEdge::Top | DemEdge::Bottom => active[3],
        DemEdge::Left | DemEdge::Right => active[1],
    };
    for segment in 0..res - 1 {
        if (skip_start && segment == 0) || (skip_end && segment == res - 2) {
            continue;
        }
        let boundary = segment_postings(
            mesh,
            boundary_src,
            edge,
            segment,
            res,
            x0,
            z0,
            step,
            dem_half_extent,
            boundary_grid_resolution,
            origin_xz,
            origin_y,
            boundary_normal_epsilon,
        );
        let inner_start = edge_inner_vertex(edge, segment, res);
        let inner_end = edge_inner_vertex(edge, segment + 1, res);
        for pair in boundary.windows(2) {
            push_triangle_up(mesh, indices, inner_start, pair[0], pair[1]);
        }
        if let Some(&last) = boundary.last() {
            push_triangle_up(mesh, indices, inner_start, last, inner_end);
        }
    }
}

fn stitch_corner<B: HeightSource>(
    mesh: &mut TileMesh,
    indices: &mut Vec<u32>,
    boundary_src: &B,
    corner: Corner,
    res: usize,
    x0: f64,
    z0: f64,
    step: f64,
    dem_half_extent: f64,
    boundary_grid_resolution: usize,
    origin_xz: [f64; 2],
    origin_y: f64,
    boundary_normal_epsilon: f64,
) {
    let (horizontal, vertical, horizontal_segment, vertical_segment, corner_vertex, diagonal) =
        match corner {
            Corner::TopLeft => (DemEdge::Top, DemEdge::Left, 0, 0, 0, res + 1),
            Corner::TopRight => (
                DemEdge::Top,
                DemEdge::Right,
                res - 2,
                0,
                res - 1,
                res + res - 2,
            ),
            Corner::BottomLeft => (
                DemEdge::Bottom,
                DemEdge::Left,
                0,
                res - 2,
                (res - 1) * res,
                (res - 2) * res + 1,
            ),
            Corner::BottomRight => (
                DemEdge::Bottom,
                DemEdge::Right,
                res - 2,
                res - 2,
                res * res - 1,
                (res - 2) * res + res - 2,
            ),
        };
    let mut horizontal_points = segment_postings(
        mesh,
        boundary_src,
        horizontal,
        horizontal_segment,
        res,
        x0,
        z0,
        step,
        dem_half_extent,
        boundary_grid_resolution,
        origin_xz,
        origin_y,
        boundary_normal_epsilon,
    );
    let mut vertical_points = segment_postings(
        mesh,
        boundary_src,
        vertical,
        vertical_segment,
        res,
        x0,
        z0,
        step,
        dem_half_extent,
        boundary_grid_resolution,
        origin_xz,
        origin_y,
        boundary_normal_epsilon,
    );
    if matches!(corner, Corner::TopRight | Corner::BottomRight) {
        horizontal_points.reverse();
    }
    if matches!(corner, Corner::TopLeft | Corner::TopRight) {
        vertical_points.reverse();
    }

    // Walk the two measured edges through their shared corner and opposite
    // grid vertex. A fan at that opposite vertex fills exactly this corner cell.
    let mut boundary = horizontal_points;
    boundary.push(diagonal as u32);
    boundary.extend(
        vertical_points
            .into_iter()
            .take_while(|&v| v != corner_vertex as u32),
    );
    let center = diagonal as u32;
    for index in 0..boundary.len() {
        let a = boundary[index];
        let b = boundary[(index + 1) % boundary.len()];
        push_triangle_up(mesh, indices, center, a, b);
    }
}

fn push_triangle_up(mesh: &TileMesh, indices: &mut Vec<u32>, a: u32, b: u32, c: u32) {
    let [ax, _, az] = mesh.positions[a as usize];
    let [bx, _, bz] = mesh.positions[b as usize];
    let [cx, _, cz] = mesh.positions[c as usize];
    let cross_y = (bz - az) * (cx - ax) - (bx - ax) * (cz - az);
    if cross_y.abs() <= f32::EPSILON {
        return;
    }
    if cross_y > 0.0 {
        indices.extend_from_slice(&[a, b, c]);
    } else {
        indices.extend_from_slice(&[a, c, b]);
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
        // A flat source has no LOD height gap, so it needs no invented seam
        // wall. The surface remains exactly the authored grid.
        assert_eq!(m.positions.len(), res * res);
        assert_eq!(m.indices.len(), (res - 1) * (res - 1) * 6);
        assert!(m.positions[..res * res].iter().all(|p| p[1] == 0.0));
        // Flat → up normals.
        assert!(m.normals.iter().all(|n| n[1] > 0.99));
    }

    #[test]
    fn edge_masks_describe_only_the_grid_perimeter() {
        let dem = ramp_dem();
        let res = 5;

        let interior = bake_tile_mesh(
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
        assert_eq!(interior.positions.len(), res * res);
        assert!(interior.edge_masks.iter().enumerate().all(|(i, mask)| {
            let ix = i % res;
            let iz = i / res;
            mask == &[
                (iz == 0) as u8 as f32,
                (iz == res - 1) as u8 as f32,
                (ix == 0) as u8 as f32,
                (ix == res - 1) as u8 as f32,
            ]
        }));

        let corner = bake_tile_mesh(
            &dem,
            &dem,
            Square {
                center: [50.0, 50.0],
                half: 50.0,
            },
            res,
            100.0,
            [0.0, 0.0],
            0.0,
        );
        assert_eq!(corner.positions.len(), res * res);
    }

    #[test]
    fn tile_has_no_fabricated_seam_geometry() {
        let dem = ramp_dem();
        let res = 5;
        let mesh = bake_tile_mesh(
            &dem,
            &dem,
            Square {
                center: [0.0, 0.0],
                half: 80.0,
            },
            res,
            100.0,
            [0.0, 0.0],
            0.0,
        );

        assert_eq!(mesh.positions.len(), res * res);
        assert_eq!(mesh.indices.len(), (res - 1) * (res - 1) * 6);
    }

    #[test]
    fn dem_perimeter_uses_every_posting_and_has_closed_corner_topology() {
        let res_dem = 9;
        let half = 100.0f32;
        let spacing = (2.0 * half) / (res_dem as f32 - 1.0);
        let mut heights = vec![0.0; res_dem * res_dem];
        // High edge relief between the coarse tile's corner/edge vertices is
        // the condition that opened the visible globe-to-surface corner gaps.
        for ix in 0..res_dem {
            let x = -half + ix as f32 * spacing;
            heights[ix] = (40.0 * (x * 0.11).sin()) as f64;
            heights[(res_dem - 1) * res_dem + ix] = (35.0 * (x * 0.09).cos()) as f64;
            heights[ix * res_dem] = (32.0 * (x * 0.13).cos()) as f64;
            heights[ix * res_dem + res_dem - 1] = (28.0 * (x * 0.15).sin()) as f64;
        }
        let boundary = HeightGrid {
            res: res_dem,
            half_extent: half,
            heights,
        };
        let flat = flat_dem();
        let tile_res = 5;
        let mesh = bake_tile_mesh_with_boundary(
            &flat,
            &flat,
            &boundary,
            Square {
                center: [0.0, 0.0],
                half: half as f64,
            },
            tile_res,
            half as f64,
            [0.0, 0.0],
            0.0,
            res_dem,
        );

        // The nonlinear postings skipped by the coarse five-vertex edge are
        // present at their authored heights, including the four corner bands.
        for edge in [DemEdge::Top, DemEdge::Bottom, DemEdge::Left, DemEdge::Right] {
            let mut edge_vertices: Vec<(f32, u32)> = mesh
                .positions
                .iter()
                .enumerate()
                .filter_map(|(index, position)| {
                    let on_edge = match edge {
                        DemEdge::Top => (position[2] + half).abs() < 1.0e-4,
                        DemEdge::Bottom => (position[2] - half).abs() < 1.0e-4,
                        DemEdge::Left => (position[0] + half).abs() < 1.0e-4,
                        DemEdge::Right => (position[0] - half).abs() < 1.0e-4,
                    };
                    on_edge.then_some((
                        match edge {
                            DemEdge::Top | DemEdge::Bottom => position[0],
                            DemEdge::Left | DemEdge::Right => position[2],
                        },
                        index as u32,
                    ))
                })
                .collect();
            edge_vertices.sort_by(|a, b| a.0.total_cmp(&b.0));
            edge_vertices.dedup_by(|a, b| (a.0 - b.0).abs() < 1.0e-4);
            assert_eq!(edge_vertices.len(), res_dem, "missing boundary posting on {edge:?}");

            for &(along, vertex) in &edge_vertices {
                let (x, z) = match edge {
                    DemEdge::Top => (along as f64, -(half as f64)),
                    DemEdge::Bottom => (along as f64, half as f64),
                    DemEdge::Left => (-(half as f64), along as f64),
                    DemEdge::Right => (half as f64, along as f64),
                };
                let expected = square_boundary_height_at(
                    &boundary,
                    x,
                    z,
                    half as f64,
                    res_dem,
                )
                .expect("boundary source sample");
                assert!((mesh.positions[vertex as usize][1] as f64 - expected).abs() < 1.0e-4);
                assert_eq!(mesh.positions[vertex as usize], mesh.morph_targets[vertex as usize]);
                assert_eq!(mesh.normals[vertex as usize], mesh.morph_normals[vertex as usize]);
                let posting_spacing =
                    square_boundary_posting_spacing(half as f64, res_dem).unwrap();
                let expected_normal = normal_at_bounded(
                    &boundary,
                    x,
                    z,
                    posting_spacing,
                    half as f64,
                )
                .map(|component| component as f32);
                assert_eq!(mesh.normals[vertex as usize], expected_normal);
            }

            let mut edge_counts = std::collections::HashMap::<(u32, u32), usize>::new();
            for triangle in mesh.indices.chunks_exact(3) {
                for (a, b) in [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ] {
                    let key = if a < b { (a, b) } else { (b, a) };
                    *edge_counts.entry(key).or_default() += 1;
                }
            }
            for pair in edge_vertices.windows(2) {
                let a = pair[0].1;
                let b = pair[1].1;
                let key = if a < b { (a, b) } else { (b, a) };
                assert_eq!(edge_counts.get(&key), Some(&1), "open or multiply drawn segment on {edge:?}");
            }
        }

        let mut edge_counts = std::collections::HashMap::<(u32, u32), usize>::new();
        let mut covered_area = 0.0_f64;
        for triangle in mesh.indices.chunks_exact(3) {
            let points = [
                mesh.positions[triangle[0] as usize],
                mesh.positions[triangle[1] as usize],
                mesh.positions[triangle[2] as usize],
            ];
            let [a, b, c] = points;
            covered_area += 0.5
                * (((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0]))
                    .abs() as f64);
            for (a, b) in [
                (triangle[0], triangle[1]),
                (triangle[1], triangle[2]),
                (triangle[2], triangle[0]),
            ] {
                let key = if a < b { (a, b) } else { (b, a) };
                *edge_counts.entry(key).or_default() += 1;
            }
        }
        for ((a, b), count) in edge_counts {
            let [ax, _, az] = mesh.positions[a as usize];
            let [bx, _, bz] = mesh.positions[b as usize];
            let tolerance = 1.0e-4;
            let is_boundary = ((ax + half).abs() < tolerance
                && (bx + half).abs() < tolerance)
                || ((ax - half).abs() < tolerance && (bx - half).abs() < tolerance)
                || ((az + half).abs() < tolerance && (bz + half).abs() < tolerance)
                || ((az - half).abs() < tolerance && (bz - half).abs() < tolerance);
            assert_eq!(count, if is_boundary { 1 } else { 2 }, "non-manifold edge {a}-{b}");
        }
        assert!(
            (covered_area - (2.0 * half as f64).powi(2)).abs() < 1.0e-2,
            "boundary strips and corners cover {covered_area} m², expected the complete square"
        );

        assert!(
            mesh.indices
                .iter()
                .all(|&index| (index as usize) < mesh.positions.len())
        );
    }

    /// Regression fixture for the normal carried by a geomorphed tile.
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

        // THE CONTRACT. At every parent-lattice vertex a fully-morphed child
        // must shade exactly like its PARENT — the tile that would be drawn
        // instead. Interior child vertices interpolate those parent vertex
        // normals, just as the parent mesh's rasterizer does across its grid.
        let step = region.side() / (res as f64 - 1.0);
        let pstep = 2.0 * step;
        let x0 = region.center[0] - region.half;
        let z0 = region.center[1] - region.half;
        let mut worst_contract = 0.0f32;
        for iz in (0..res).step_by(2) {
            for ix in (0..res).step_by(2) {
                let sx = x0 + (ix & !1) as f64 * step;
                let sz = z0 + (iz & !1) as f64 * step;
                // What a parent tile computes for this vertex, verbatim.
                let hx = dem.height_at(sx + pstep, sz) - dem.height_at(sx - pstep, sz);
                let hz = dem.height_at(sx, sz + pstep) - dem.height_at(sx, sz - pstep);
                let (nx, nz) = (-hx / (2.0 * pstep), -hz / (2.0 * pstep));
                let len = (nx * nx + 1.0 + nz * nz).sqrt();
                let want = [nx / len, 1.0 / len, nz / len];
                let got = m.morph_normals[iz * res + ix];
                let cosang =
                    (want[0] as f32 * got[0] + want[1] as f32 * got[1] + want[2] as f32 * got[2])
                        .clamp(-1.0, 1.0);
                worst_contract = worst_contract.max(cosang.acos().to_degrees());
            }
        }
        assert!(
            worst_contract < 0.5,
            "morph normal must be the PARENT lattice's own central difference at \
             the snapped point (off by up to {worst_contract:.2} deg) — otherwise \
             a fully-morphed tile shades unlike the parent it stands in for"
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

    /// The morph target remains inside the position-derived bounds, so Bevy's
    /// automatic AABB remains valid while the vertex stage applies the morph.
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
        // The parent is a smoothed version of the child, so the displaced geometry stays
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
    fn parent_surface_preserves_edge_coordinates() {
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
                // The parent surface has the same X/Z coordinates as the child;
                // only its height is coarsened. This is what makes a stitched
                // fine edge coincide with the coarse neighbour.
                assert!((m.morph_targets[i][0] - m.positions[i][0]).abs() < 1e-3);
                assert!((m.morph_targets[i][2] - m.positions[i][2]).abs() < 1e-3);
                // The ramp is linear, so bilinear parent interpolation is still
                // exactly height == world X at every child coordinate.
                assert!((m.morph_targets[i][1] - (x0 + ix as f64 * step) as f32).abs() < 1e-2);
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
        // height == world x on this ramp for every regular-grid vertex.
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
