# DEM-to-globe continuation handoff

Date: 2026-09-30
Fixture: Summer Space School Twin, `sim/scenes/traverse_apollo15.usda`

## Result and measured cause

The four crop corners join the exterior without the vertical wall in close
oblique production captures. Measured DEM values, mission queries, celestial
physics radii, and collider construction are unchanged.

The shared square boundary sampler must vary X on north/south edges and Z on
west/east edges. Using the edge's fixed Z coordinate as its interpolation
coordinate produced an approximately 82 m erroneous jump near `(499, -499)`.
Persisted visual tiles also carried that bad boundary into subsequent launches.
Visual tile sampling/layout revision 13 is part of the content key, so those
incompatible bakes cannot be consumed. No cache deletion or alternate loader
is required.

## Current architecture

- `lunco-terrain-core` owns the posting interval and piecewise-linear perimeter
  sampler. Both immutable collar signals and visual DEM boundary bakes consume
  that lattice; authoritative in-crop mission height queries retain their oracle.
- `lunco-celestial-spatial` prepares one immutable perimeter signal asynchronously
  for the admitted crop revision. Each corner has one value and gradient. One
  width, sized from maximum measured relief, applies to all four sides.
- `lunco-terrain-globe` preserves the native inner lattice through the first
  outward posting, then reduces exterior ring density. Radial tessellation
  resolves the edge continuation and cubic fade; unsupported budgets return a
  structured preparation error. The outer boundary joins the analytic sphere.
- `lunco-terrain-surface` owns material composition and collar visibility.
  `GlobeLod` supplies the current composed body appearance; the authored
  declaration entity supplies lint provenance. Rhai admits matching declared
  and reflected interfaces, waits for reflection, and holds on a mismatch.
  Unadmitted collars remain hidden; globe cutout admission reads readiness and
  visibility. There is no substitute material path.
- `ShaderLook` owns shadow intent. Generated globe/collar looks derive their
  render-shell intent through one owner; the DEM cache binder composes its
  active self-shadow producer into the same typed intent. The material binder
  alone writes `NotShadowCaster`. Dynamic objects can still shadow the DEM.
- `TerrainLodStatus` provides bounded collar and CPU-retained DEM boundary mesh
  pages through one reader, including active-physics positions and morph
  targets. Interior tile meshes stay GPU-only; boundary CPU retention is
  included in the bounded mesh-cache estimate. These reads are explicit
  diagnostics, not frame or physics work.

## Production evidence

Current Apollo owned session: PID 485558, API port 49401, production debug
binary in tutorials. PID, executable, and working directory were verified.
The shorter exterior is 199.484508641 m on every side, down from approximately
595 m. Its mesh has 12,020 vertices, approximately 50 percent fewer than the
24,044-vertex smoothing baseline. The native boundary is never decimated.

`terrain_surface_continuation.rhai` waits for collar admission and settled DEM
streaming, then compares the complete native collar and rendered DEM perimeter
against mission heights. Apollo PASS: 14 checks, including displaced-boundary
rejection. Both native boundary counts are 2,036; maximum errors are
0.0000165915 m and 0.0000190327 m; boundary morph displacement is zero.
Log: `target/apollo-short-final.log`. Captures inspected:
`target/apollo-short-final-corner.png` and
`target/apollo-short-final-overview.png`.

The repository-owned `lunar_dem_continuation.usda` composes the production DEM
appearance fixture and solar system without root or terrain coordinates.
Its site resolves automatically to Moon, latitude/longitude/height zero.
`lunar_dem_georeferenced.usda` uses the same fixture and authors latitude 23,
longitude -47 on the DEM; the scene site resolves to those coordinates.
Both headful production runs PASS all 14 checks with 12,048 vertices and
200.664625012 m exterior widths. Logs: `target/lunar-dem-auto.log` and
`target/lunar-dem-georef.log`. The fixtures were authored and saved with the
USD document commands, and saved source was read back.

## Checks and limits

- `cargo test -p lunco-celestial-spatial collar_ -j 4`: four tests PASS.
- `cargo test -p lunco-usd-sim-celestial scene_site_anchor_ -j 4`: PASS;
  inline composed-schema cases cover omitted coordinates, nonzero coordinates,
  and malformed coordinate rejection.
- `cargo build -p lunco-luncosim -j 4`: PASS.
- The headless `luncosim test` attempt for the graphics fixture failed with no
  collar admission: that host has no GPU/image loader and cannot validate this
  rendered-boundary contract. Its exit code was 1; it is not a passing gate.
  The headful production verdicts above supply the graphics evidence.
- Direct standalone fixture launches also reported an existing ambiguity in
  automatic Twin policy discovery under assets/scenes/tests. Application
  policies remained installed and the headful geometry verdicts passed.
- Root Terrain prims use the same scene-site decoder. Explicit root anchor
  fields own the scene frame; otherwise one lunar DEM and a composed Moon
  declaration derive it. Multiple active crops remain rejected explicitly.
- The exterior has a 0.60 relief-grade sizing target and half-posting radial
  interpolation tolerance; sphere-edge chord tolerance stays quarter-posting.
  It is visual closure, not measured mission terrain or a physical slope limit.
- No per-frame smoothing, extra texture samples, or new mesh path is added.
  No comparative FPS gain is claimed. Shadows remain enabled and DEM appearance
  stays distinct. Current-time lighting does not establish photometric or
  complete mission acceptance.

## Recovery and integration scope

Original work remains recoverable from stash
`21b2280d88b05f0c95aa4fc59875644511a8a712`.
Integrate only these reviewed changes into local main and tutorials. No push
is authorized. Other sessions and worktrees remain under their current owners.
