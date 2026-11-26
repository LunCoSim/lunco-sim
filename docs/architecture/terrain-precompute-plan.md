# Terrain precompute measurements

> Historical design evidence, not an active implementation plan or a CI test.
> The measurements used a private DEM checkout; repeat explicitly on the target
> terrain when sampling or bake math changes.

The authoritative runtime contract is [progressive startup](terrain-substrate.md#progressive-startup).
One root and camera selection prepare concurrently; only incomplete branches
retain hidden ancestors. Near morphing and geometry use shared visual bands,
while collider sampling stays independent. Final derived shading is background
work; recording requires its current product. The measurements below explain
why an exhaustive offline tile pyramid is not the default.

## Measured: there is no sparsity

The original sparse-set measurement, against the real moonbase DEM (3200², ±8000 m, 5.00 m
posting), error floor 0.05 m, max depth 8:

```
 depth        tiles     node m   cumulative
     0            1    16000.0        0.2 MB
     1            4     8000.0        0.8 MB
     2           16     4000.0        3.2 MB
     3           64     2000.0       13.0 MB
     4          256     1000.0       52.3 MB
     5         1024      500.0      209.5 MB
     6         4096      250.0      838.2 MB
     7        16384      125.0     3352.9 MB
     8        65536       62.5    13411.6 MB

TOTAL 87381 tiles ≈ 13411.6 MB   (157 KB/tile at 49² verts)
uniform tree to depth 8 = 87381 tiles — sparsity is 1.0× smaller
```

**Every node clears the error floor, so the tree refines fully everywhere.** Lunar regolith is
fractal — rough at every scale — so there is no "flat enough to stop" region. The premise that
error-driven refinement yields a content-shaped sparse tileset is **false for this terrain**.

The measured dense tree would require 13.4 GB for one site, so on-demand
geometry and content-addressed caching remain necessary. These are historical
mesh sizes, not the current cache format or resident tile count.

## Historical coarse-pyramid bake benchmark

The original bake-time measurement used the real DEM and `bake_tile_mesh` single-threaded:

```
 depth    tiles     total ms      ms/tile
     0        1          0.6         0.61
     4      256        159.7         0.62

TOTAL 341 tiles in 236 ms serial (0.69 ms/tile)

native, 32 cores (ideal)   ≈   7 ms
native, 4 cores (weak)     ≈  59 ms
wasm, single-threaded MAIN ≈ 236 ms   (no worker pool)
wasm at 3x native slowdown ≈ 707 ms
```

These are measurements and arithmetic projections for a 341-tile pyramid;
they are not evidence that every depth is resident or that browser CPU work
is nonblocking. See the current substrate and production review for actual
startup and frame-time evidence.
