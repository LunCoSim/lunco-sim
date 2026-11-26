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

## Craterlet LOD appearance and stability

The streamed surface reserves complete camera-centred minimum-detail zones
within the existing tile budget. The visual craterlet reconstruction uses the
bowl diameter for its Nyquist fade; physical sampling and contact lattices
remain unchanged. An unavailable replacement mesh cannot publish an unselected
ancestor over ready finer geometry. Constrained-cover preparation uses ancestor
walks and indexed required nodes rather than scanning entire covers for each pin.

Production Summer Space School / rover workshop evidence (Linux, RTX 5060
Laptop GPU, private hardware-accelerated Weston output, 1600 × 1000,
High quality, main view and StereoLeft feed):

- Both covers settled at 223 and 169 tiles, respectively, with all selected
  tiles drawn and 3.05 cm finest postings. Mesh accounting was 121.4 MiB.
  A 68-second paused sample retained all 392 exact tile identities; moving the
  workshop camera by 2 m produced a new settled main cover and kept the feed
  independent. Images show continued medium-distance crater relief without
  increasing the tile budget. Both coarser and finer bands remain represented.
- Median application FPS was 83.8 while paused (34 samples), and 75.2 while
  playing (15 samples). Runtime diagnostics contained zero errors or warnings.
  These are measured on the stated native output, not a cross-platform FPS
  guarantee or an apples-to-apples speedup against an earlier binary.
- The first revised visual-cache bake took about one minute after world admission;
  the warm run settled both covers within one second of world admission.
  The authored `terrain_stationary_cover.rhai` gate passed its three checks.
- Evidence: `/tmp/luncosim-lod-final/` (proof frames and sampled covers) and
  `/tmp/luncosim-lod-warm/` (authored verdict and playing measurements).

## Progressive startup and architecture acceptance

The native stream starts root geometry and projected cover planning concurrently,
then admits exact, disjoint 2:1-restricted partition replacements in camera-near
order. Hidden ancestor entities retire as their branches publish. Camera-near
readiness means the main-world published drawable partition, not a GPU pixel
receipt or just completed CPU meshes. Full
derived rasters and horizon-cache work use bounded background admission;
source and quality leases fence completion. The worker and finalized camera
demand use one certificate-aware selection signature, so a paused camera can
converge without another simulation tick. Offline recording requires the
requested material product and applicable bound shadow cache; debounce is
independent of the recording clock.

Production Summer Space School / rover workshop tests on Linux RTX 5060
Laptop GPU, 1600 × 1000, High quality, independent main and StereoLeft outputs
(port 3743; isolated visual cache followed by the same warm cache):

| Milestone (seconds from launch) | Cold visual cache | Warm visual cache |
| --- | ---: | ---: |
| First reported root partition (initial feed) | 5.515 | 4.746 |
| Physical world ready | 7.534 | 6.437 |
| Both views reported fully drawn (screenshot verified afterward) | 8.318 | 7.267 |
| Final shading and current geometry | 38.114 | 7.376 |
| Median settled frame time (240 frames) | 10.14 ms | 9.97 ms |
| 95th-percentile settled frame time | 11.77 ms | 11.07 ms |
| Median settled FPS | 98.65 | 100.25 |

The first main root-publication count is not pixel visibility; the query milestones for complete drawn partitions were 0.784 s and 0.830 s after
world readiness; subsequent screenshot inspection verified pixels. These timings
are publication timings, not exact GPU readback timestamps. The cold full shading upgrade preserved all 392 precise
drawn tile identities and camera poses. Both views (223 and 169 tiles)
settled at 3.05 cm finest postings, with zero pending selection or
presentation work and no mesh upload refusals. Approximate resident geometry
was 123.0 MiB; 151.6 MiB was accounted including reclaimable cache entries.
The remaining 110 refused refinements were discretionary tile-budget limits,
not missing camera coverage. The cold cache contained both 256² and 2048²
derived raster artifacts. Both runs passed the authored per-presentation-tick
stationary-cover verdict and had zero runtime diagnostic errors or warnings.
Warm numbered recording produced three actual frames and reported no pending
visual preparation or capture failure.

The production build passed, as did 241 terrain-surface, 123 terrain-core,
two precompute and five capture tests (three existing ignored tests). Evidence:
`/tmp/luncosim-terrain-audit/cold2/` and
`/tmp/luncosim-terrain-audit/warm2/`. The frame-time sample is a native
settled interactive run, not a browser/physics-running performance claim.
The browser still uses cooperative CPU preparation on the page executor.

## Post-integration audit (2026-11-26)

The follow-up removes the timed render-entity retirement shim against the
change-driven native extraction contract, caches probes by source and lattice,
accounts shared mesh eviction incrementally, and avoids constructing ancestor
unions in settled frames. Terminal current-lease bake failures now report
diagnostics instead of leaving numbered recording waiting indefinitely.
Horizon configuration changes retire obsolete source leases and published maps.
Malformed cached geometry is rejected before admission.

The integrated renderer and current Summer Space School Twin (`235c862`)
were tested on the RTX 5060 at 1600×1000, port 3743. This Twin revision
opens two feeds as well as the main viewport (three covers, 567 drawn tiles),
unlike the earlier two-view measurements above. The warm run reused the
successful cold run's terrain cache. Simulation was paused for the settled
frame-time measurement.

| Milestone (seconds from launch) | Cold | Warm |
| --- | ---: | ---: |
| Initial feed root publication | 4.532 | 5.771 |
| Physical world ready | 6.998 | 9.413 |
| Complete main-world draw partition | 7.583 | 10.900 |
| Interactive 256² raster preview | 7.750 | — |
| Current 2048² raster and geometry | 42.076 | 11.074 |
| Median settled frame time (240 frames) | 15.95 ms | 18.73 ms |
| 95th-percentile settled frame time | 27.79 ms | 30.59 ms |
| Median settled FPS | 62.71 | 53.39 |

All 567 exact draw identities and camera poses were unchanged in the cold
preview-to-final upgrade. Recording armed before final shading produced no
numbered frames during the preparation hold, then delivered two full-quality
frames; the warm test also delivered two. Both runs passed the three authored
stationary-cover checks, with no runtime diagnostic errors or warnings. Both
independent feeds were retired, the probe lattice changed from 9 to 11, and
the remaining viewport settled and rendered correctly. The cold preview,
numbered frames and warm final and retuned screenshots show uniform near
crater detail and a continuous distant cover. The warm initial draw-partition
screenshot was still missing main-view terrain and route; the next status
sample 0.174 s after the partition query was followed by a screenshot showing
both. This is why main-world publication is not a GPU
visibility receipt. No settled disappearance was observed in the inspected
numbered frames; this is not exhaustive per-pixel coverage of every frame.

An additional warm run reached 72.61 median FPS with the three views; there
was variable concurrent system load, and the older two-view results are not
an isolated performance comparison. Earlier attempts also encountered private
Wayland compositor failures; the matched runs completed on fresh GPU-backed
private displays. The production build and 380 focused tests passed (three
existing ignored). Evidence: `/tmp/luncosim-terrain-audit/postmerge-cold4/`,
`/tmp/luncosim-terrain-audit/postmerge-warm3/`, and
`/tmp/luncosim-terrain-audit/postmerge-warm2/`.

### Initial viewport presentation repair

The workbench loading state owns its backdrop, not camera render admission.
The selected physically admitted window camera now prepares terrain and GPU
pipelines during initial simulation preparation, alongside independent feeds.
The exposed-camera contract separately prevents screen-space labels, pointer
presses, gizmo interaction, and terrain/placement editing behind the backdrop.
No timer or synchronous pipeline compile is introduced.

The production driver accepts `--viewport-probe 1000 330 1200 440` for the
1600×1000 workshop view. It measures luminance occupancy and variation in
the same unobscured ground rectangle at first main-view publication, complete
draw publication, and final shading. The two original warm captures failed
the probe (0% lit early; 95.17% final). With the repair, both final cold and
warm runs passed (100% lit coarse cover in the first main-view screenshot;
95.17% lit complete/final, with terrain luminance variation in all three).
The complete detailed covers published 0.804 s and 0.616 s after physical
world readiness, respectively. Cold shading still upgraded 256² to 2048²
without changing the 567 drawn tile identities; numbered capture held until
that upgrade completed. The warm run again passed feed retirement and probe
retuning. All 166 camera/workbench/editor tests passed and both production
runs passed stationary-cover and diagnostic gates. The test fixtures also
register the actual design panel and handle headless egui texture deltas.

Evidence: `/tmp/luncosim-terrain-audit/startup-final-cold/` and
`/tmp/luncosim-terrain-audit/startup-final-warm/`. These runs used the native
renderer pins at `a68de0251` with the presentation repair. Other worktree
compilation competed for system resources, so the observed 41.85/30.80 median
FPS is not an isolated speed comparison. Neither quality nor simulation
cadence was reduced.

The combined production build at `a8e3d959d`, including main's newer retained
native CPU buffer renderer (`c7582e05`), also passed both startup probes and
stationary-cover gates on port 3743. The cold detailed cover published 0.376 s
after world readiness; final current shading arrived at 49.199 s from launch,
preserving all 567 tile identities and gating two numbered frames. The warm
cover published 1.898 s after world readiness, with two numbered frames and
successful feed retirement/probe retuning. Both runs had zero runtime
diagnostic errors/warnings. Their median FPS observations were 59.57 cold and
23.11 warm; the latter overlapped a release Rust link/codegen, video encode and
other simulator sessions. The acceptance driver now allows a configurable
deadline to collect all 240 fresh frames instead of implicitly enforcing a
20 FPS sampling floor. The initial warm attempt passed pixels/stationary
coverage but exceeded the original twelve-second sampling deadline.
Evidence: `/tmp/luncosim-terrain-audit/startup-integrated-cold/` and
`/tmp/luncosim-terrain-audit/startup-integrated-warm2/`.

### Presentation and preparation contract audit

The main viewport's exposed-camera authority is now shared by editing,
picking, and window capture. Workbench hosts without a scene viewport retain
their independent pointer routing. Offscreen numbered capture requires the
actual output image and all occupied color-phase pipelines; a request entity
owns each delivered frame. Terminal terrain diagnostics abort recording before
another sampling ticket can be issued.

Static horizon maps and pending bakes retire on mesh-handle or authored
resolution changes. Mesh admission distinguishes a temporarily full cache from
a single mesh larger than the entire configured ceiling. The latter is a
terminal diagnostic until that lease or ceiling changes, not a repeated bake.
The streamer also reuses its live mesh inventory and skips pose collection
when no camera targets request terrain.

The native driver samples the first observed exposed viewport independently of
tile draw counts, requires a nonempty exact main-view cover, and checks pixels
in numbered capture and after feed retirement. This is sampled evidence, not
an assertion that every GPU frame has been inspected. The focused six-package
test pass covered 363 passing tests and one existing ignored test.

The production build on `8906e198a` plus the audit changes passed both the
cold-terrain and warm workshop runs on port 3743 at 1600×1000. The first
sampled exposed main viewport had 100% lit probe pixels; complete and final
views had approximately 95%, with the numbered captures matching the final
probe. The complete main-world partition followed physical readiness by
1.67 s cold and 0.74 s warm. All 567 tile identities remained stable from
interactive preview to final cold shading, three stationary-cover checks
passed on both runs, and warm feed retirement/probe retuning passed. Median
settled frame times were 16.98 ms cold and 17.94 ms warm (58.90 and 55.74 FPS).
Final cold shading arrived 124.12 s after launch under shared machine load;
this is not an isolated performance improvement measurement. Runtime
diagnostics reported zero errors and warnings in both windowed runs.

On the offscreen `sun_material_precision.usda` fixture, the actual image
target acknowledged ready pipelines and the recorder started 38 ms later,
then produced two valid 640×360 images and exited. The workshop's offscreen
run at 640×360 did not complete during that pass: the pipeline receipt arrived,
but two planetary covers and one terrain selection/presentation remained
pending despite 208/208 resident tiles. The next audit resolved the shared
pose-owner defect and verified real workshop recordings, below.

Evidence: `/tmp/luncosim-terrain-audit/deep-audit-warm/`,
`/tmp/luncosim-terrain-audit/deep-audit-cold/`,
`/tmp/luncosim-terrain-audit/deep-audit-offscreen-fixture/`, and
`/tmp/luncosim-terrain-audit/deep-audit-offscreen/`.

### Shared windowless presentation and bake lease audit

Offscreen uses the same authored viewport and origin selection as a windowed
host. The separate image-render camera is a leased output consumer; the
selected authored pose owner drives planetary LOD and terrain precision. The
recorder checks the target's occupied opaque, masked, transmissive and
transparent render phases. Generic GPU readbacks are not capture requests.
Duplicate starts reject without altering an existing take; output/readback
failures preserve the terminal outcome through teardown and CLI exit.

Static and streamed horizon maps retire only the image owned by their source
lease. Invalid streamed heightfield and visibility jobs stop at a row boundary; DEM
removal retires queued derived work and its published maps. Scoped heightfield
sampling and invariant AO ray setup preserve the authored quality contract.
The AO regression matched the direct integral bit for bit. The seven-package
owner suite compiled and, after correcting the new readback test fixture's
deferred-command flush, passed 440 distinct tests with one existing ignored
test; the AO regression passed separately.

The production build on `0e56812e1` plus the audit changes includes main's
retained native camera/light visibility revision. On port 3743, the formerly
blocked workshop at 640×360 admitted its canonical viewport source and a complete
205/205 tile capture partition, with no selection, presentation or derived
pending work. It recorded six inspected images and exited successfully. The
competing start returned HTTP 409 without creating a second destination. An
invalid CLI output directory independently exited with status 1 and zero frames.

The 1600×1000 cold-terrain windowed run published the first root 0.329 s after
visual demand and the complete draw partition 0.908 s after physical world
readiness. Preview published at 32.51 s and final shading at 93.34 s from
launch; all 567 tile identities were unchanged. The first exposed viewport,
numbered frames, final viewport, and post-retirement/retune image passed the
ground-pixel probe, with zero runtime diagnostic errors or warnings and three
passing stationary-cover checks. Median settled FPS was 43.71 (22.88 ms;
p95 150.31 ms) on the shared machine, not an isolated performance comparison.

Evidence: `/tmp/luncosim-terrain-audit/review2-offscreen3/`,
`/tmp/luncosim-terrain-audit/review2-invalid-output/`, and
`/tmp/luncosim-terrain-audit/review2-cold/`. The separately tracked remaining
issues are in [open-terrain-preparation-and-visibility.md](open-terrain-preparation-and-visibility.md).

The combined tree at `7a0687c9f` also passed the workshop offscreen gate. Its
warm windowed run exposed a late-stop GPU lifetime defect: Bevy still owned an
already-submitted `Screenshot` when the recorder retired it. The recorder now
cancels its logical numbered lease without deleting a submitted window request;
Bevy's completion and `Captured` cleanup retain physical ownership. The
deterministic late-stop regression and all eight recording-readiness tests
passed, and the exact windowed rerun completed recording, feed retirement and
probe retuning. Detailed cover followed readiness by 0.333 s; all 567 tiles
remained stable, the initial and numbered pixel probes passed, and diagnostics
were empty. Warm median FPS was 63.37 (15.78 ms; p95 24.43 ms). The prior failed
run's 12.84 FPS overlapped heavy compiler and swap activity; it is not a clean
comparison. The final binary also repeated the offscreen gate: 205/205 complete
tiles, six numbered images, competing-start rejection and successful own exit.
Evidence: `/tmp/luncosim-terrain-audit/review2-late-stop-warm/` and
`/tmp/luncosim-terrain-audit/review2-final-offscreen/`.

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
