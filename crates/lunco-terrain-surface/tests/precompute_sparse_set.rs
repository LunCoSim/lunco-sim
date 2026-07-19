//! Step 1 of `docs/architecture/terrain-precompute-plan.md`: **measure the sparse set**.
//!
//! Decides the three numbers the precompute design depends on, against the REAL moonbase
//! DEM rather than an estimate:
//!
//! 1. how many tiles error-driven refinement actually wants, and at what depth distribution
//! 2. what that costs on disk (fixes `N`, the always-resident coarse base)
//! 3. how long baking it takes
//!
//! Sparsity comes from the selector's own rule: a node refines when the camera is inside
//! `range_factor · error(depth)`, so a node whose MEASURED surface error is ~0 (flat mare)
//! has a ~0 refine range and its children can never be selected — there is no point baking
//! them. Crater fields keep their error and go deep. That makes the baked set content-shaped
//! instead of a uniform grid, which is what makes precompute tractable at all: the full tree
//! to depth 8 is ~65k tiles.
//!
//! Read-only and `#[ignore]`d — it loads a multi-megabyte DEM off the twin and walks the
//! whole tree, so it is a diagnostic you run deliberately:
//!
//! ```text
//! cargo test -j2 -p lunco-terrain-surface --test precompute_sparse_set -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use lunco_terrain_core::error::measure_node_error;
use lunco_terrain_surface::{
    height_grid_from_geotiff, quadtree::Square, QuadCoord, SurfaceOracle,
};

/// The twin this measures against. Skipped (not failed) when absent, so the suite still runs
/// on a checkout without the twin.
fn dem_dir() -> PathBuf {
    PathBuf::from("/home/rod/Documents/models/moonbase/twin/terrain/connecting_ridge")
}

/// Mirrors `stream_viz`'s node-error probe resolution so the measurement matches what the
/// selector will actually do at runtime.
const NODE_ERROR_PROBE_RES: usize = 9;

/// Vertices per side of a baked tile (`stream_viz::TILE_RES`).
const TILE_RES: usize = 49;

/// Bytes per baked tile: `res²` vertices × (position + morph target + normal + uv) f32s,
/// plus the index buffer. Matches `tile_cache::tile_mesh_to_bytes`'s layout.
fn tile_bytes(res: usize) -> usize {
    let verts = res * res;
    let per_vert = (3 + 3 + 3 + 2) * 4;
    let indices = (res - 1) * (res - 1) * 6 * 4;
    verts * per_vert + indices
}

/// Refinement is pointless below the error the metric can even express — a node flatter than
/// this contributes nothing by splitting, so its subtree is never baked. Metres.
const ERROR_FLOOR_M: f64 = 0.05;

#[test]
#[ignore = "diagnostic: loads the moonbase twin DEM and walks the full quadtree"]
fn sparse_set_depth_distribution() {
    let dir = dem_dir();
    let tif = dir.join("materials/textures/heightmap.tif");
    if !tif.exists() {
        eprintln!("SKIP: twin DEM not present at {}", dir.display());
        return;
    }

    let bytes = std::fs::read(&tif).expect("heightmap.tif reads");
    let grid = height_grid_from_geotiff(&bytes).expect("geotiff decodes");

    let half = grid.half_extent as f64;
    let spacing = grid.spacing() as f64;
    println!(
        "\nDEM: {}² samples, ±{half:.0} m, {spacing:.2} m posting",
        grid.res
    );

    // Base only. The analytic layers (craters/overzoom) ADD error, so a DEM-only measurement
    // is a LOWER bound on the sparse set — stated rather than silently assumed.
    let oracle = SurfaceOracle::bare(std::sync::Arc::new(grid));
    let root = Square { center: [0.0, 0.0], half };

    // Max depth the runtime can select (`stream_viz::MAX_DEPTH`).
    const MAX_DEPTH: u8 = 8;

    let mut per_depth = [0usize; (MAX_DEPTH as usize) + 1];
    // Nodes whose error cleared the floor, i.e. worth refining INTO -> their children exist.
    let mut refined_per_depth = [0usize; (MAX_DEPTH as usize) + 1];

    // Iterative walk; only descend where the parent's measured error justifies it.
    let mut stack = vec![(QuadCoord::ROOT, root)];
    while let Some((coord, region)) = stack.pop() {
        per_depth[coord.depth as usize] += 1;
        if coord.depth >= MAX_DEPTH {
            continue;
        }
        // Gate over-zoom synthesis at the probe's own spacing, exactly as the runtime does.
        let probe_step = region.side() / (NODE_ERROR_PROBE_RES - 1) as f64;
        let limited = oracle.detail_limited(probe_step);
        let err = measure_node_error(&limited, region, NODE_ERROR_PROBE_RES);
        if err <= ERROR_FLOOR_M {
            continue; // flat enough that splitting changes nothing
        }
        refined_per_depth[coord.depth as usize] += 1;
        let h = region.half * 0.5;
        for c in coord.children() {
            let cx = region.center[0] + if c.x % 2 == 0 { -h } else { h };
            let cz = region.center[1] + if c.z % 2 == 0 { -h } else { h };
            stack.push((c, Square { center: [cx, cz], half: h }));
        }
    }

    println!("\nsparse set (error floor {ERROR_FLOOR_M} m, max depth {MAX_DEPTH}):");
    println!("{:>6} {:>12} {:>10} {:>12}", "depth", "tiles", "node m", "cumulative");
    let mut total = 0usize;
    let mut cumulative_bytes = 0usize;
    for d in 0..=MAX_DEPTH as usize {
        let n = per_depth[d];
        if n == 0 {
            continue;
        }
        total += n;
        cumulative_bytes += n * tile_bytes(TILE_RES);
        let node_m = (2.0 * half) / (1u64 << d) as f64;
        println!(
            "{:>6} {:>12} {:>10.1} {:>10.1} MB",
            d,
            n,
            node_m,
            cumulative_bytes as f64 / (1024.0 * 1024.0)
        );
    }
    println!(
        "\nTOTAL {total} tiles ≈ {:.1} MB on disk ({} KB/tile at {TILE_RES}² verts)",
        (total * tile_bytes(TILE_RES)) as f64 / (1024.0 * 1024.0),
        tile_bytes(TILE_RES) / 1024
    );

    // The always-resident coarse base: what does depth <= N cost?
    println!("\ncoarse base candidates (always resident, never evicted):");
    let mut running = 0usize;
    for n in 0..=6usize.min(MAX_DEPTH as usize) {
        running += per_depth[n];
        println!(
            "  N={n}: {running:>6} tiles ≈ {:>7.1} MB",
            (running * tile_bytes(TILE_RES)) as f64 / (1024.0 * 1024.0)
        );
    }

    // A uniform (non-sparse) tree, for contrast — this is what makes sparsity load-bearing.
    let uniform: usize = (0..=MAX_DEPTH as usize).map(|d| 1usize << (2 * d)).sum();
    println!(
        "\nuniform tree to depth {MAX_DEPTH} would be {uniform} tiles ≈ {:.1} GB — \
         sparsity is {:.1}× smaller",
        (uniform * tile_bytes(TILE_RES)) as f64 / (1024.0 * 1024.0 * 1024.0),
        uniform as f64 / total as f64
    );
}
