//! Step 2 of `docs/architecture/terrain-precompute-plan.md`: **how long does the coarse base
//! take to bake?**
//!
//! [Step 1](precompute_sparse_set.rs) fixed the coarse base at depths `0..=N` with `N = 4` —
//! 341 tiles, 52 MB — as the always-resident fallback that makes a not-yet-baked deep tile
//! degrade to *blurry* rather than to *black*. This measures what producing it costs, which
//! decides the delivery mechanism:
//!
//! - fast enough → bake at scene open, no shipped artifact, no cache-invalidation story
//! - too slow → must ship prebaked with the twin (and then a live terrain edit has to
//!   re-bake the touched region incrementally)
//!
//! Timed against the REAL bake (`bake_tile_mesh`, same band-limiting the runtime applies),
//! single-threaded, so the number is the honest serial cost. Native divides it across the
//! task pool; **wasm cannot** — `AsyncComputeTaskPool` has no threads there, so the serial
//! number IS the wasm number, on the main thread.
//!
//! ```text
//! cargo test -j2 --release -p lunco-terrain-surface --test precompute_bake_time -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Instant;

use lunco_terrain_surface::{
    height_grid_from_geotiff, quadtree::Square, tile_mesh::bake_tile_mesh, HeightSource, QuadCoord,
    SurfaceOracle,
};

fn dem_dir() -> PathBuf {
    PathBuf::from("/home/rod/Documents/models/moonbase/twin/terrain/connecting_ridge")
}

/// Vertices per side of a baked tile (`stream_viz::TILE_RES`).
const TILE_RES: usize = 49;

/// Deepest level of the always-resident coarse base (step 1 fixed this at 4).
const COARSE_N: u8 = 4;

#[test]
#[ignore = "diagnostic: bakes the full coarse base off the moonbase twin DEM"]
fn coarse_base_bake_time() {
    let dir = dem_dir();
    let tif = dir.join("materials/textures/heightmap.tif");
    if !tif.exists() {
        eprintln!("SKIP: twin DEM not present at {}", dir.display());
        return;
    }

    let bytes = std::fs::read(&tif).expect("heightmap.tif reads");
    let grid = height_grid_from_geotiff(&bytes).expect("geotiff decodes");
    let half = grid.half_extent as f64;
    println!("\nDEM: {}² samples, ±{half:.0} m, {:.2} m posting", grid.res, grid.spacing());

    // Base only — the analytic layers add per-sample cost, so this is a LOWER bound.
    let oracle = SurfaceOracle::bare(std::sync::Arc::new(grid));

    // Walk depths 0..=COARSE_N, baking every node exactly as the runtime would.
    let mut per_depth_ms = [0.0f64; (COARSE_N as usize) + 1];
    let mut per_depth_n = [0usize; (COARSE_N as usize) + 1];
    let mut total_verts = 0usize;

    let t_all = Instant::now();
    let mut stack = vec![(QuadCoord::ROOT, Square { center: [0.0, 0.0], half })];
    while let Some((coord, region)) = stack.pop() {
        let d = coord.depth as usize;

        // Same band-limiting the tile cache applies: this tile's Nyquist, and the parent
        // lattice's for the morph targets.
        let step = region.side() / (TILE_RES.max(2) - 1) as f64;
        let limited = oracle.detail_limited(2.0 * step);
        let parent_limited = oracle.detail_limited(4.0 * step);
        let origin_y = oracle.height_at(region.center[0], region.center[1]);

        let t = Instant::now();
        let mesh = bake_tile_mesh(
            &limited,
            &parent_limited,
            region,
            TILE_RES,
            half,
            region.center,
            origin_y,
        );
        per_depth_ms[d] += t.elapsed().as_secs_f64() * 1000.0;
        per_depth_n[d] += 1;
        total_verts += mesh.positions.len();

        if coord.depth < COARSE_N {
            let h = region.half * 0.5;
            for c in coord.children() {
                let cx = region.center[0] + if c.x % 2 == 0 { -h } else { h };
                let cz = region.center[1] + if c.z % 2 == 0 { -h } else { h };
                stack.push((c, Square { center: [cx, cz], half: h }));
            }
        }
    }
    let wall_ms = t_all.elapsed().as_secs_f64() * 1000.0;

    println!("\ncoarse base bake (N={COARSE_N}, {TILE_RES}² verts/tile, single-threaded):");
    println!("{:>6} {:>8} {:>12} {:>12}", "depth", "tiles", "total ms", "ms/tile");
    let mut tiles = 0usize;
    for d in 0..=COARSE_N as usize {
        if per_depth_n[d] == 0 {
            continue;
        }
        tiles += per_depth_n[d];
        println!(
            "{:>6} {:>8} {:>12.1} {:>12.2}",
            d,
            per_depth_n[d],
            per_depth_ms[d],
            per_depth_ms[d] / per_depth_n[d] as f64
        );
    }

    let mean_ms = wall_ms / tiles as f64;
    println!("\nTOTAL {tiles} tiles in {wall_ms:.0} ms serial ({mean_ms:.2} ms/tile)");
    println!("  vertices baked: {total_verts}");

    // Delivery verdict. Native parallelises across the task pool; wasm does not.
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    println!("\ndelivery:");
    println!("  native, {cores} cores (ideal)   ≈ {:.0} ms", wall_ms / cores as f64);
    println!("  native, 4 cores (weak)         ≈ {:.0} ms", wall_ms / 4.0);
    println!("  wasm, single-threaded MAIN     ≈ {wall_ms:.0} ms  (no worker pool)");
    println!(
        "  wasm at 3x native slowdown     ≈ {:.0} ms",
        wall_ms * 3.0
    );
    println!(
        "\nverdict: bake-at-open is viable iff the wasm number is under a scene-open budget \
         (~2 s); otherwise the coarse base must ship prebaked with the twin."
    );
}
