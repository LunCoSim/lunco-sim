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

Owned session: PID 363774, API port 49401, `tutorials/target/debug/luncosim`.
The PID, executable, and working directory were verified before control.
`/api/ready` reported ready, no hold/fault, and zero pending work. That session
was stopped with `Exit` after capture.

`assets/scenarios/tests/terrain_surface_continuation.rhai` runs on the admitted
DEM prim through `RunScenarioAsset`. Its production verdict was PASS (14 checks),
including rejection of an 80 m displaced boundary sample:

- Native collar postings checked: 2,036; maximum error 0.0000165915 m.
- Native rendered DEM postings checked: 2,036; maximum error 0.0000190327 m.
- Rendered DEM boundary morph displacement: 0 m.
- All four exterior widths: 594.531803748 m.
- Collar: 24,044 vertices, approximately 71% fewer than the 82,832-vertex baseline.

Evidence log: `target/apollo-current-bake-contract.log`.

| Corner in east/south physics coordinates | Eye | Target | Capture |
|---|---|---|---|
| (+X, -Z), reported corner | (540, -1940, -570) | (490, -1998, -490) | `target/apollo-current-bake-low-se.png` |
| (-X, -Z) | (-560, -1835, -580) | (-490, -1916, -490) | `target/apollo-current-bake-low-sw.png` |
| (+X, +Z) | (560, -1848, 580) | (490, -1928, 490) | `target/apollo-current-bake-low-ne.png` |
| (-X, +Z) | (-560, -1830, 580) | (-490, -1910, 490) | `target/apollo-current-bake-low-nw.png` |

Shadows stayed enabled. The fixture selects computer time because it has no
root epoch; these captures establish geometric joining, not a controlled
photometric comparison or complete mission acceptance. No FPS claim is made.

## Focused checks

- Square boundary regression: demonstrated failure before correction, PASS
  after correction and after centralizing the posting interval.
- `cargo test -p lunco-celestial-spatial -p lunco-terrain-globe collar_ -j 4`:
  3 celestial and 6 globe math/geometry tests passed.
- `cargo test -p lunco-terrain-surface shadow_cache_ -j 4`: 2 lifecycle tests passed.
- Production shader asset gate: PASS, 67 checks, six ticks, exit 0; includes
  negative interface cases. Log: `target/terrain-final-shader-contracts.log`.
- Production build: `cargo build -p lunco-luncosim -j 4`.
- Skill catalogue: 43 skills validated. Final diff whitespace and touched Rust
  formatting are checked before commit.

## Recovery and integration scope

Original work remains recoverable from stash
`21b2280d88b05f0c95aa4fc59875644511a8a712`.
Integrate only these reviewed changes into local main and tutorials. No push
is authorized. Other sessions and worktrees remain under their current owners.
