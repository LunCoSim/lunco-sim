# DEM-to-globe continuation handoff

Date: 2026-09-30
Fixture: Summer Space School Twin, `sim/scenes/traverse_apollo15.usda`

## Current result

The measured DEM, its mission queries, and physics colliders are unchanged.
Only the exterior visual continuation and its material composition change.
The dark corner notch is absent in the new overhead capture. A straight edge
band remains visible, so this is not complete visual or mission acceptance.

## Current contract

- `lunco-celestial-spatial` prepares one immutable measured perimeter signal
  asynchronously and admits it only for the matching input revision. Each
  exterior sample reads one nearest boundary point; the corner does not sum
  overlapping side relief. Four side widths define a rectangular fade.
- `lunco-terrain-globe` keeps native samples on the inner ring, reduces
  exterior ring density, and clips front-hemisphere globe tiles in their
  affine orthographic chart. The exterior meets the analytic render sphere.
- `lunco-terrain-surface` composes the DEM base colour, authored raster weight,
  surface maps, and lunar photometry into the USD-selected body appearance.
  `ShaderLookSourceInterface` publishes typed shader defaults for omitted
  parameters. The shared lighting helpers apply the direct-sun response;
  the collar performs no heightfield shadow march. Invalid source material
  parameters report a structured continuation fault.
- Interior DEM rendering is preserved. Rust geometry and GPU material
  composition remain generic; Rhai chooses interface admission.
- Terrain uniforms remain 256 bytes; blueprint uses 240 bytes. Geometry
  preparation does not run on the physics cadence.

## Evidence

- Terrain geometry tests: 14 passed.
- Celestial handoff tests: 9 passed before the final outer-ring density sizing
  adjustment; the production build includes that adjustment.
- Integrated production build passed with `cargo build -p lunco-luncosim
  --bin luncosim -j 4`.
- Production `shader_asset_contracts.usda` gate passed at six ticks, including
  negative shader/interface checks and the 256-byte uniform bound.
- Skill catalogue validation passed (43 skills); `git diff --check` passed.
- Owned Apollo session: port 49401, PID 234429, checkout `tutorials`, executable
  `target/debug/luncosim`. API readiness was healthy, no pending work or
  runtime fault. The session was shut down through `Exit` after capture.
- Camera eye `(-450, -1700, -450)`, target `(-450, -1918, -450)`.
- Capture: `target/apollo-surface-photometry-corner.png`.
- The earlier integrated run reported a rover leaving finite physics bounds.
  No mission-level acceptance is established by these visual checks.
- The scene uses computer time because it omits a root epoch; captures are
  therefore not a controlled comparison of celestial illumination.

## Integration and recovery

Terrain geometry commit: `8026b1fca`. Existing main merge resolved in
`b7d4ea5e1`, preserving stricter failed-load cleanup and deterministic prim
selection. Terrain integrated in `ca02fc8ab`, then tutorials fast-forwarded.
The subsequent material correction is committed with this handoff.

The original tracked/staged/untracked state remains recoverable from stash
`21b2280d88b05f0c95aa4fc59875644511a8a712`. No branch was pushed.

## Remaining bounded investigation

Trace the visible straight edge band through the actual collar material and
inner-ring shading in the production scene. Establish the consumed material
values before further shader changes. Keep the measured DEM and colliders
unchanged. A close oblique view and a controlled scene epoch are still needed
for visual acceptance; performance screenshots alone are not profiling.
