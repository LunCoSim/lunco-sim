# Camera terrain fidelity and collision implementation

Status: native Linux implementation validated through focused mechanism tests and
production navigation, simultaneous-view and scene-replacement gates. Existing regional DEM, material, dataset and processing
work in this checkout is preserved.

## Current ownership

| Contract | Owner and real consumer |
|---|---|
| Physical terrain | The immutable `SurfaceOracle`, authored collider settings, deterministic collider ring and native dynamics. Camera quality does not choose physical sampling or support residency. |
| Consumed views | `SceneViewDemand` is published by viewport, feed, preview and capture owners. The renderer admits active writing outputs with real dimensions, isolates private render layers and withdraws retired leases. Terrain consumes finalized snapshots after camera and viewport publication. |
| Visual terrain | `LodTiles` owns a separate cover, fallback, stitch state and morph context per consumed camera. Source/node/resolution mesh bakes and cache handles are shared. Visual bands sample the source independently of physics filtering. |
| View metric | Perspective uses output pixels, FOV and three-dimensional distance. Orthographic uses finalized area and pixels per metre within its finite footprint, with a projected posting-size floor at every range for both projections. Orthographic covers use static parent-edge stitching and discrete zoom transitions. Custom metrics reject visibly. |
| Resource admission | Per-view covers honor tile limits. Asynchronous cover/bake admission and uploads stay bounded. The source-global cache counts unique live, cached and retiring mesh identities before upload; invalidated live generations remain accounted. The existing byte setting retains GPU-upload semantics; CPU payload is reported separately. |
| Source camera safety | `GridSurfaceQuery::sweep_sphere` returns clear, contact, initial overlap or unavailable. Conservative bounds prune work; actual source columns provide witnesses. Movement casts the physical sphere and applies skin as directional contact pullback. Native prop casts use checked Avian adapters and exclude native DEM realizations. |
| Presentation safety | `DrawTerrainQuery` checks the solid heightfield under retained fine/parent mesh hulls, actual stitch uniforms, correlated morph intervals and the GPU precision budget. Coarse geometry requests preparation. Current finest contacts join source/prop contact-plane projection, with every segment recast. Candidate cover, morph and stitch publication is separately guarded; unsafe replacements retain the admitted draw and report `presentation_pending`. Invalid geometry or exhausted capacity rejects visibly. |
| Pending navigation | `TerrainCameraTargets` carries frame-qualified continuous corridor requests across render frames. Follow preparation holds orientation and translation together; changing its requested endpoint coalesces required nodes. Source, camera position, output or projection changes retire that lease. Scroll additionally retains its exact input/settings request and retries on presentation revisions. Explicit source-certified upward overlap recovery remains possible with props checked. Lateral input generates no automatic upward correction. |
| Lens clipping | USD owns source clipping ranges. Perspective avatars retain those floors while celestial adaptation improves distant precision; fixed observers and mounted sensors retain their authored projection. Checked conversion to the native projection happens only at the rendering boundary. |
| Shadows | Output geometry participates in native CPU/GPU visibility. Directional shadow views use their originating camera cover; shared point/spot maps use the explicit `ShadowLodOrigin` output. Authored light masks and shared props retain their owner. |

Visual posting counts are odd so 2:1 parent grids nest. The error estimator compares
source samples to the indexed anti-diagonal mesh triangles. Regional root expansion
adds its exact power-of-two depth offset, preserving local metric visual spacing
without inheriting collider settings.

The source's geometric error is a property of the tile. Its projected error is
camera dependent: perspective uses `error * pixels_high / (2 * distance * tan(fov_y / 2))`.
Moving closer, narrowing the FOV or increasing output resolution requests more
detail. Coarse probes can filter small relief, so projected posting density also
drives discovery at every range. Finalized clip volumes use conservative full-source
height bounds rather than filtered probes. A retained conservative clip envelope
uses pixel/projection-based angular tolerances and hysteresis for both invalidation
and clipping. This preserves readiness under mounted-camera jitter while responding
to meaningful telephoto pans and roll changes. The immutable source caches its global enclosure, computed
on native preparation workers. Synchronous web preparation only peeks at that cache
and keeps uncertain nodes eligible; this change has no web runtime acceptance. Orthographic error uses `error * pixels_per_metre`; zoom changes that
scale, while translation along the viewing axis does not. A selected cover can
stay unchanged at a finest-detail floor, between refinement thresholds, or when
its explicit resource budget refuses further splits.

## Lifecycle and policy review

Output owners decide when an image is consumed; terrain does not infer consumption
from a rover or maintain a parallel camera registry. Quality values remain in the
existing authored Rhai rendering policy. Projection math, balanced-cover invariants,
collision math and resource admission remain Rust engine mechanisms. No new
scenario-selection hook or duplicate material-selection policy is introduced.

View removal cancels its exact queued cover work and retires its geometry. Render
layers are quarantined through extraction retirement before reuse. Scene-derived
camera demands, stream status and preparation targets use `SceneResource` teardown;
streamed tiles and exterior collar geometry carry `SceneOwned`. Camera preparation never gates physics admission.

Draw-safety calculations implement the shared CDLOD fine/parent deformation
contract. An arbitrary authored vertex program needs its own geometry provider;
these checks do not certify arbitrary shader displacement. Mounted cameras retain
their authored mounts; inspect their reported terrain-local clearance. Finite GPU
precision and explicit quality/resource limits remain admission boundaries.

## Verification evidence

Evidence directory: `.cache/camera-terrain-fix/`.

- Core: 122 tests passed, two ignored (`certificate-core-tests.log`), including indexed
  triangle error and orthographic distance/zoom and constrained-cover morph tests.
- Surface: 231 tests passed, one ignored (`view-envelope-final-tests.log`), including
  corridor persistence, correlated morph sweeps, solid-prism separation, child
  publication lattices, distant telephoto discovery, retained clip-envelope
  readiness under native ULP jitter, orthographic axial-distance invariance,
  conservative clip bounds and strict shallow source sweeps at the unchanged work budget and tolerance.
- Raster: 24 passed; bake: 33 passed (`certificate-obstacle-tests.log`,
  `certificate-bake-tests.log`). Native spatial precision: three passed, including
  same-grid astronomical rebasing and insufficient-precision/controller negatives
  (`solar-spatial-tests.log`). The camera owner publishes its exact origin binding
  (one focused test, `portable-lunco_usd_bevy_camera-tests.log`).
- Camera math: 18 passed; avatar envelope/policy: four passed; source lens and
  reprojection: 53 passed (`latest-camera-core-tests.log`,
  `latest-avatar-policy-tests.log`, `lens-tests-accepted.log`).
- Camera output leases/layers/default shadow origin: four focused tests passed
  (`views-final.log`). Native shadow candidate/mask mechanisms: two passed
  (`shadows-final.log`). Avatar lifecycle: 26 passed; presentation readiness: six
  passed (`navigation-avatar.log`, `navigation-presentation.log`).
- Full renderer unit suite: 96 passed (`direct-lunco_render_bevy-tests.log`).
  Bare ECS fixtures register their required asset/shadow resources and initialize
  valid explicit quality settings.
- Python: the four modified API drivers parse.
- Production native-collider scene: four authored checks passed. The camera
  descended from 3 m to 0.0271 m, within its declared envelope
  (`verified-native-prop.log`).
- Production independent views: 14 authored checks and six invalid-quality
  rejection checks pass on the Apollo 15 rover workshop. The driver requires a
  native-contact drive, an unchanged stationary main cover, independent mounted
  output, feed retirement and zero runtime diagnostics. Main/feed images are
  inspected for crater relief, seams and native shadows. Final snapshots and
  measurements are in `final-envelope-views/validation.json`. In the final run,
  the rover drove 7.080 m; the main cover stayed at 178 tiles while the mounted
  output received 190 tiles. Both finest postings were 0.030517578125 m.
- Production scene replacement with quality changes: six authored checks passed,
  including physical support and independent camera cover readiness after two
  replacements (`final-envelope-restart.log`).
- Production navigation: all 58 authored checks passed in one owned session
  (`final-envelope-navigation.log`): follow 8, rapid motion 5, low free flight 11,
  capacity refusal/recovery and stationary coarsening 12, origin rebasing 8,
  explicit collision policy 6, and distant zoom 8. Default low-ground clearance
  was about 0.027 m; rebasing crossed both native cell boundaries with zero
  measured altitude drift. An unchanged 1 km-high camera refined from depth 4
  to 8 when focal length changed from 18 to 1500, reducing posting spacing from
  1.3020833333333333 m to 0.08138020833333333 m with identical physical support.
  Wide/telephoto images were inspected. Final diagnostics, GPU/panic scan and
  owned-session Exit passed. Follow gates allow a declared bounded preparation
  wait while work is pending; these are correctness and liveness checks, not
  frame-time or preparation-latency acceptance.
- Integration with local main `d8baaf79e` passed the root production compile,
  the same 231 surface tests and five dataset manifest tests. The regional probe
  example also builds. The live 315-command schema regenerated the command
  reference while preserving main's stable Twin identity contract. All 84
  navigation/view/quality/replacement assertions passed again; current evidence
  is under `integration/accepted-*/`. The four required Apollo artifacts were
  prepared through `ProcessDataset` from cached pinned inputs before these gates.
  Pose-refusal assertions use the reflected camera sweep tolerance rather than
  requiring cross-frame floating-point pose reads to be bit-identical.
- Builds use the normal profile, target directory, `-j 4` and sccache.

Required production acceptance includes native prop clearance, low free-flight
clearance and lateral drift beyond rover support, follow/rapid motion, stationary
main-camera independence during a physical drive, simultaneous mounted-camera feed,
feed retirement, paused navigation and scene teardown. Inspect captured images for
crater relief, seams, route/trail registration and native shadows. Status counts
alone do not prove any image result. No FPS target or platform-wide result is claimed. Native Windows/macOS, web and
planet-wide multiview rendering were not exercised by these gates.

## External architecture references

[Cesium Native view integration](https://cesium.com/learn/cesium-native/ref-doc/rendering-3d-tiles.html)
and [selection/ancestor readiness](https://cesium.com/learn/cesium-native/ref-doc/selection-algorithm-details.html)
support per-view projected error with shared content and ready ancestor retention.
The [original CDLOD implementation](https://github.com/fstrugar/CDLOD) motivates a
consistent cover, stitch and morph context. Native prop handling reuses
[Avian MoveAndSlide](https://docs.rs/avian3d/latest/avian3d/character_controller/move_and_slide/struct.MoveAndSlide.html)
projection and shape-cast mechanisms. Standard lens facts remain owned by
[UsdGeomCamera](https://openusd.org/release/api/class_usd_geom_camera.html).
GPU precision admission follows the [WGSL floating-point accuracy contract](https://www.w3.org/TR/WGSL/#floating-point-accuracy),
including its rounding freedom and inherited distance/mix accuracy.
