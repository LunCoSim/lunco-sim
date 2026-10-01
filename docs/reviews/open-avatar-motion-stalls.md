# Avatar movement and Editor stalls

## Current result

The input, shader-binding, and idle document-notification owners have been
corrected. Product measurements below use the production binary, High quality,
normal shadows and physics, and owned explicit API ports. Tracy remains a
separate diagnostic run. Polling samples do not cover every rendered frame.

## Architecture

- Scene pointer dispatch is document-scoped and subscription-driven. Idle
  movement invokes no Rhai hook; idle primary clicks perform no route queries.
  Active preview transforms stay in ECS, while placement authors USD.
- `lunco-render-bevy` shares shader-stage validation, optional uniform layout,
  and interface facts by published shader asset revision. Tile replacements
  check loaded dependencies and layout identity without parsing WGSL. Resolved
  interfaces are visited only after shader asset events. See
  [shader layers](../architecture/shader-layers-and-params.md).
- Document notification publishers check their event queues before acquiring
  mutable registry access. USD preview text painting borrows cached state.
  Idle reads therefore do not invalidate SysML requirements view models.
- Native automation uses semantic movement and distinct absolute pointer and
  relative mouse-motion streams. Rotation waits for observed native yaw.

These are generic engine mechanisms. Authored Rhai retains interaction,
selection, and scenario policy; USD retains scene facts and shader selection.

## Evidence

Summer Space School: `sim/scenes/traverse_apollo15.usda` in the external
`summer_space_school` model. The measured avatar starts at
`[-340, -1907, -340]`; the pointer is seeded in SceneView through the rover's
projected position. The October 1 scene includes the model's current authored
changes and its opted-in runtime overlay. The completed measurements needed no
explicit pose edit. Failed startup probes that observed the overlay's inactive
translation were excluded.

| Check | Evidence | Result |
| --- | --- | --- |
| Idle click hook cost | `sss-calibrated-click-before.tracy` / `sss-calibrated-click-after.tracy` | Before: 24 calls, mean 100.90 ms, max 188.32 ms. After: 19 calls, mean 1.25 ms, max 5.11 ms. Different call counts; not a matching frame comparison. |
| Movement stall attribution | `sss-pointer-crossings-oct01.tracy` | Shader rebinding took 37–41 ms at repeated crossings around 4.3 and 7.0 s; picking stayed below 3 ms. |
| Shared-source cache | `sss-shader-cache-after.tracy`, `.source.csv`, `.shader-stats.json` | No shader validation or schema parsing during movement. Rebinding near the former crossings is about 1 ms; one later rebinder outlier is 17.93 ms. |
| Unprofiled native movement | `sss-shader-cache-plain.samples.json`, `.input.log` | 228.26 m; 473 observed frames, p50 8.79 ms, p99 18.28 ms, max 20.94 ms; no samples over 40 ms. |
| Unprofiled native rotation | Same | Measured 360 degrees, 231 observed frames, p50 8.64 ms, max 15.29 ms. |
| Unprofiled SceneView clicks | Same | 24 native clicks; 461 observed frames, p50 8.42 ms, p99 14.82 ms, max 34.58 ms. |
| Scene stability | Same | Document generation 13 and all layer revisions unchanged through movement, rotation, and clicks. |
| Source-cache lifecycle seam | `shader-source-cache-test.log` | PASS: 500 consumers share facts/layout identity; invalid reload, valid repair, and removal retire the prior revision. |
| Production invalid shader | `shader-cache-negative-production.log`, `.app.log` | Existing authored `shader_fallback.rhai` verdict: `TESTS_OK 1`; runtime-invalid library shader rejected by `shader-render`, without a process crash. Probe executed after measurements in a separate owned session. |

Artifacts above are under `target/perf/`. The performance windows completed
before a display-name lookup failed in the optional negative probe; the separate
negative session uses the exact USD path and passes. No Cargo build, Tracy
export, or other simulator ran during the completed unprofiled windows.

The Editor check used `lunar-base-model/twins/astrobotic-griffin-1`, scene
`griffin_flip_visual.usda`, and five exact USD source previews. The input flags
in `griffin-editor-invalidation-before-oct01.tracy` attribute 342 phantom rebuilds
to document change detection alone (p50 54.97 ms). The after capture has no such
rebuilds. `griffin-editor-unprofiled-after-oct01.samples.json` observes p50
13.52 ms without a preview, 15–21 ms with successive Visual previews, and
16.71 ms with composed Text. Bodies/colliders/joints remain 33/33/12. That
Editor run had another workload active and does not establish an uncontended
before/after ratio.

## Remaining test infrastructure issue

The shared scene-test folder contains two nested Twin policy manifests and its
Twin bootstrap rejects that ambiguity. The standalone shader fixture therefore
did not reach its verdict; this is distinct from the passing shader-owner probe.
Fixture policy isolation remains open.

The touched route tests use named behavior-tree actions and `seq`.
`route_interaction` passes 36 checks; `route_lifecycle` emits one PASS with 110
checks after retaining the verdict helper's returned state. Its scoped verdict
does not establish a valid Twin bootstrap for the shared test folder.
