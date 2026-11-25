# High-quality realtime performance handover

At the time of these captures, High used an 8192 px directional map, four
cascades, Hardware 2×2 filter, 0.005 m depth bias, 2.0-texel normal bias, and
3000 m maximum distance. The active High profile has since changed; use the
current authored profile for present-day settings. The
LunCoSim Bevy fork did not already contain receiver-slice culling. Bevy's
renderer tracking issue identifies cascade-frustum overcoverage and lists
culling to the camera slice in light space as an optimization
([issue #12590](https://github.com/bevyengine/bevy/issues/12590)); a separate
open issue describes an exact silhouette-extrusion approach
([issue #10397](https://github.com/bevyengine/bevy/issues/10397)).

The LunCoSim fork publishes this receiver-slice culling in branch
`perf/directional-shadow-receiver-culling` at commit
`8671dfe743d284f074587862e03bcd3e0b79ecc6`. The patch projects the eight
corners of each active camera slice into cascade light space, builds their 2D
convex hull and precomputes its separating axes once per view/cascade. Each
transformed caster AABB is rejected only when its conservative light-space
projection is disjoint from that hull. Light-space depth stays unbounded so
distant upstream casters remain eligible. Filter, normal-bias and half-pixel
temporal-jitter footprints expand the hull; invalid or unsupported projections,
`NoFrustumCulling`, Temporal filtering, and PCSS soft shadows use Bevy's native
culling path. The patch adds 13 focused tests in `bevy_light` for hull
intersection, padding, jitter and fallback cases.

The earlier A/B/A used a rectangular receiver bound, before the current convex
hull SAT refinement. It used Griffin surface operations, High/2560×1440, Tracy
0.14.1 at 1 kHz, the authored `landing_handoff` qualification gate, and matched
10-second settled windows. A and A2 used the saved stock-Bevy release binary
`0bd9c31`; B used the Tracy release built with the local Bevy path patch
`afcc06a`.

| Tracy zone (mean / p95 ms) | Stock A | Patched B | Stock A2 |
| --- | ---: | ---: | ---: |
| `lunco_shadow_camera_early` | 4.4194 / 4.9951 | 3.9489 / 4.2660 | 4.2653 / 4.9615 |
| Nested `per_view_shadow_pass` | 2.9281 / 3.3119 | 2.3831 / 2.5751 | 2.8212 / 3.2827 |
| `main_opaque_pass_3d` | 2.8133 / 3.1383 | 2.8375 / 3.1570 | 2.7982 / 3.0856 |

Against the two stock runs' mean, the rectangular patch's nested shadow pass
was 17.1% lower and its early shadow zone 9.1% lower; the opaque zone differed
by 1.1%. A/B/A final images preserved the long lander shadow at the original
High settings. These measurements apply to the rectangle implementation only.
The current hull SAT code has a separate matched bracket below. Captures,
exports and images are under
`target/perf/tracy-20261108/directional-shadow-culling-high8k-4c-{a-control,b-patched,a2-control-clean}/`.

The current convex-hull source was built with the production Tracy feature and
the local `bevy_light` path override, then run on API port 4230 with Tracy port
8177. The exact Griffin scene passed `landing_handoff`; the admitted Twin source
hashes stayed unchanged, diagnostics had zero errors and warnings, and both
first-ready and final images retain the lander and its long shadow. The capture
is `target/perf/tracy-20261108/hull-custom-4230/capture.tracy` (399,616,775
bytes); its production ELF SHA256 is
`53f2b1932b29a5a852dfac5bdd4c34b99fb856bf742ceda5d7bad0b4781f9480`, retained
as `target/perf/tracy-20261108/hull-custom-4230/luncosim-hull-53f2`. The matching
Tracy 0.14.1 native sample reader reports a 1,000,000 ns
sampling period. The settled event window spans 44.041 to 55.187 seconds in the
capture receipt; these are Tracy zone timings, not whole-frame FPS.

| Tracy zone | Mean / p95 (ms) |
| --- | ---: |
| GPU `lunco_shadow_camera_early` | 3.1550 / 3.5451 |
| GPU nested `per_view_shadow_pass` | 1.6620 / 1.8673 |
| GPU `main_opaque_pass_3d` | 2.7437 / 3.0344 |
| CPU `bevy_light::check_dir_light_mesh_visibility` | 0.2306 / 0.2845 |

The workload snapshots show no competing simulator or Cargo build during the
qualified interval. Desktop processes and the background `sccache` service were
present. This single capture on application HEAD `32779e0` did not establish a
before/after gain. A follow-up capture at
`target/perf/tracy-20261108/hull-custom-4232/` is diagnostic only: a separate
tutorial simulator used about 178% CPU during that run. The capture, native
exports, receipt and images for the uncontended single run are under
`target/perf/tracy-20261108/hull-custom-4230/`. API port 4230 and Tracy port
8177 were released after that run.

### Matched stock/hull/stock bracket at application HEAD `5ae1388`

The bracket used application HEAD
`5ae138886e698b5795d6e84054732a7b062d4fb8` and the same working-diff SHA256
(`c1e3e2a8c84438199e887a1c0cb9c9c5a996ff762b141869e2aa5b8738574f8d`) in all
three runs, the same Griffin Twin HEAD
`b97c679581599195940833a25191d3cbaaa6874a`, and unchanged Twin source hashes.
Stock A and A2 used the pinned stock Bevy release binary (SHA256
`7b8449f381dbb2278b9b4a4d15c84abc91061a509343bfb23437ff8e5b28ca10`);
candidate B used the local `../bevy` branch
`perf/directional-shadow-receiver-culling`, based on `65dd8f4`, with the
one-file patch diff SHA256
`557f1ef463032f6c6beee0dd72f9ebc1de8b12442a5530405c09bbe0fbc499f9`
(candidate binary SHA256
`6d0b03d78c357bb462eb278fcc984fb50d72f9637c3cc64050df94cfa7d882e5`). The three APIs were 4233,
4234 and 4235, with Tracy ports 8179, 8180 and 8181. Every run used the same
release profile, Tracy feature, Tracy 0.14.1 collector at 1 kHz, exact
`griffin_1_surface_ops.usda` OpenFile scene, `landing_handoff` gate, High
quality profile and 2560×1440 camera target. No other simulator or Cargo build
was active in the qualified windows. The measured intervals were 40.239–51.319,
39.591–50.913 and 38.615–49.468 seconds in each trace, aligned to its OpenFile
zone and the driver's qualified sample window.

| Tracy zone (mean / p95 ms) | Stock A | Hull B | Stock A2 | B mean vs. stock mean |
| --- | ---: | ---: | ---: | ---: |
| GPU `lunco_shadow_camera_early` | 4.695 / 4.993 | 3.325 / 3.546 | 4.635 / 4.990 | −28.7% |
| GPU nested `per_view_shadow_pass` | 3.111 / 3.306 | 1.750 / 1.867 | 3.068 / 3.303 | −43.3% |
| GPU `main_opaque_pass_3d` | 2.787 / 2.989 | 2.736 / 2.838 | 2.768 / 2.945 | −1.5% |
| GPU `early prepass` | 0.586 / 0.617 | 0.575 / 0.600 | 0.577 / 0.610 | −1.2% |

After directional shadow work, the opaque pass is the next-largest GPU zone in
candidate B at 2.736 / 2.838 ms (mean / p95); the early prepass is 0.575 /
0.600 ms and bloom is 0.259 / 0.271 ms.

### Opaque shader attribution — 2026-11-09

A diagnostic-only patch in the local `../bevy-020-tracy` checkout adds Tracy
GPU spans around contiguous opaque mesh draw groups, labeled by fragment shader
asset path. It does not change rendering behavior. The production release was
built at application HEAD `1db09023c2c03c0e8e7a40038bf3521179ceb339` with the
existing worktree diff SHA256
`f3eea1a46e602b933a601812dfe9fe35d38c41ffc348349ac51c08f149279a3f`; its
binary SHA256 is
`eb16140d48d6af42472e647141453501636dd695f4e55fd0d94893d17caf6459`.

The exact Griffin `OpenFile` scene passed `landing_handoff` on API port 4357
with Tracy 0.14.1 on port 8357 at 1 kHz, High quality, a 2560×1440 target and
pointer passthrough. The driver qualified a 10.8-second settled window; source
hashes stayed unchanged and final diagnostics reported zero errors and warnings.
The capture, receipt, native shader symbols, runtime log and images are under
`target/perf/opaque-shader-griffin-20261109/`. The first-ready image shows the
surface before the lander is visible; the final image retains the lander and its
long shadow. Both owned ports were released.

The capture-wide GPU export contains 1,542 `main_opaque_pass_3d` events
(2.634 ms median, 4.730 ms p95). The terrain shader group
`lunco://shaders/terrain_layered.wesl` appears in 1,347 events (2.356 ms median,
2.691 ms p95); its summed group time is 2,886 ms, about 75% of the opaque-phase
sum. These spans measure GPU time around draw groups selected by that shader
asset, not the shader's instruction time in isolation. The capture-wide window
includes scene loading and settling. During qualification, another simulator
on API 4199 used about 156% CPU and the existing simulator on API 4155 used
about 33% CPU, so these figures attribute work but do not support a before/after
comparison or FPS claim.

The trace points the next investigation at the terrain material's fragment
work. It does not support removing the Sun or adding a global-illumination
crate: native shadow culling already reduces the shadow pass, while terrain
shading is now the largest identified opaque draw group. No terrain rendering
change has been made from this diagnostic capture.

A matched Tracy A/B/A then evaluated a temporary shader fast path for the
terrain's `csm_far <= 0` case, moving the per-fragment distance calculation
behind the active horizon-shadow branch. The candidate was removed after the
comparison; no rendering change remains. The three 10-second GPU windows were
aligned to the same settled `OpenFile` marker. Main opaque pass and terrain
draw-group median / p95 times (ms) were A `2.701 / 4.539` and
`2.363 / 2.427`, B `2.741 / 4.585` and `2.403 / 2.603`, and A2
`2.689 / 4.537` and `2.353 / 2.531`. Nested early-shadow times stayed
comparable: A `1.860 / 3.702`, B `1.852 / 3.675`, A2 `1.863 / 3.694`;
early-prepass times were A `0.598 / 0.610`, B `0.592 / 0.595`, A2
`0.591 / 0.602`. Controls A and A2 agree closely; B is slightly slower in
the terrain group and opaque pass. The branch did not improve the measured
work, so it is rejected. This is a Tracy-zone result, not an FPS claim or a
complete explanation of terrain shader cost. Captures and receipts are under
`target/perf/terrain-blend-distance-ab-20261109/{a-control,b-fast-path,a2-control-clean}/`;
the first A2 attempt overlapped a sibling Cargo build and is excluded.

The shader-attribution startup log also reports two lit, unscoped scene
`DistantLight` prims, leaving the intended key light ambiguous. The Twin
source was not changed; lighting comparisons need authored content with one
explicitly selected key light before they can support a light-removal decision.

The stock A/A2 early-shadow means differ by 1.3%, and the nested shadow-pass
means by 1.4%; their p95 values are similarly stable. The hull candidate lowers
those two GPU zones while the opaque pass remains nearly flat. In CPU Tracy
zones, `check_dir_light_mesh_visibility` measured 0.204 / 0.262 ms in A,
0.228 / 0.286 ms in B, and 0.194 / 0.249 ms in A2 (mean / p95). Its
`system_commands` zone measured 0.354 / 0.540, 0.288 / 0.420 and 0.312 / 0.483
ms. These are instrumented zone costs, not whole-frame FPS.

All three runs passed the landing handoff, preserved source hashes, and ended
with structured `RuntimeDiagnostics` at 0 errors / 0 warnings. Visual inspection
of all three final images shows the same lander, surface and long ground shadow
at High settings. The startup logs also repeat unresolved authored references
for `environments/lunar_surface_base.usda`,
`environments/south_pole_surrogate.usda`, `vehicles/flip_v5.usda` and
`vehicles/griffin_1.usda`, plus an ambiguous multiple-sun warning. Treat these
as a shared content limitation: the bracket compares the content that loaded,
not the missing referenced prims or a fully resolved lighting setup.

The captures, native Tracy CSV exports, receipts and images are under
`target/perf/tracy-20261108/hull-ab/{stock-a,candidate-b,stock-a2}/`. All API
and collector ports were released. The bracket used a temporary Cargo path
override for candidate B. After the bracket, the one-file patch was committed
and pushed to the LunCoSim Bevy fork as
`perf/directional-shadow-receiver-culling` at
`8671dfe743d284f074587862e03bcd3e0b79ecc6`; the app now pins `bevy_shader`,
`bevy_render`, `bevy_light` and `bevy_pbr` to that revision. The 150 FPS
objective remains open: the matched Tracy zones establish a shadow-pass
reduction, not end-to-end FPS.

### Published fork production capture

The locked release build using the published Bevy commit succeeded. Its
production binary SHA256 is
`b34b8aeb3c84cb93b9c761e877efe377bd9c9fcc89b6252facde57f56dacb917`. The
`griffin_1_surface_ops.usda` scene opened through `OpenFile` on API port 4250,
at High quality and 2560×1440, with Tracy 0.14.1 on port 8200 at 1 kHz. The
matching native sample reader verified a 1,000,000 ns period across 122,837 CPU
samples, with zero missing stacks. The authored `landing_handoff` qualification
passed; the source tree stayed unchanged, final diagnostics had zero errors and
warnings, and the final image retains the lander and its long ground shadow.

Tracy recorded 11,180,539 zones over 53.16 seconds. The capture is
`target/perf/tracy-20261108/pinned-8671dfe-api4250/capture.tracy` (95,829,945
bytes); the matching `tracy-csvexport` exited successfully and parsed 30,553
GPU-zone events. The receipt, CSV and images are alongside the capture. API
4250 and collector port 8200 were released. A separate simulator remained
active during this run (PID 1030428, API 4199; 31.3% CPU in the qualified
workload snapshot), so this capture verifies the pinned production build and
Tracy integration but is not a performance comparison. The uncontended matched
bracket above remains the performance evidence.

### Custom-port Tracy diagnosis — 2026-11-08

The exact Griffin `OpenFile` workload also completed on API port 4253 with the
matching Tracy 0.14.1 collector on port 8203, High quality, a 2560×1440 camera,
`landing_handoff`, and pointer passthrough. The release diagnostic binary kept
`opt-level=3` and thin LTO while retaining symbols (`-C strip=none`); its SHA256
is `8157789e817ca0294e726662747dd172197856f0135192d640c39c5477f657a5`. The
exact executable, native maps, ELF metadata, Tracy capture, CPU samples, and
native CPU/GPU zone exports are under
`target/perf/tracy-20261108/custom-port-4253-symbols/`.

Tracy recorded 11,616,219 zones over 51.14 seconds. The native sample reader
verified a 1,000,000 ns period for 127,522 samples, with no missing stacks.
Static addresses were resolved against this exact ELF and its captured load
map. The source snapshot remained unchanged; `landing_handoff` passed and final
runtime diagnostics reported zero errors and warnings. API 4253 and collector
8203 were released.

Simulator PID 1030428 on API port 4199 remained active at about 20.6% CPU
during qualification, so use this capture for diagnosis; the uncontended
bracket above remains the comparison evidence. Tracy's
capture-wide GPU-zone means / p95 values were 3.003 / 3.554 ms for
`lunco_shadow_camera_early`, 2.600 / 3.087 ms for `main_opaque_pass_3d`,
1.612 / 5.741 ms for `main_transparent_pass_3d`, 0.530 / 0.630 ms for `early
prepass`, and 0.257 / 0.268 ms for bloom. These are individual instrumented
zones; their values do not sum to frame time or establish end-to-end FPS. The
first-ready image precedes `landing_handoff`; the final image shows the landed
vehicle and long shadow.

The Rhai surface projection zone averaged 1.130 ms across 1,582 calls. A
zone-filtered native sample set contained 1,664 samples in 1,550 projection
zones; among 1,419 samples resolved to the application ELF, `_mi_page_malloc_zero`
appeared in 527 stacks. Rhai's sorted-map conversion also appears in the
resolved stacks (`DedupSortedIter`: 120; `hook_to_dynamic`: 18; key sorting: 28).
This attributes a substantial part of the hook cost to building Rhai maps from
complete scalar output facts. The production Griffin policy reads those facts
with both bracket and dot access, and its contract test removes a field to check
the missing-output diagnostic; preserve those semantics when addressing this
cost.

`modelica_integrate_macro_step` averaged 0.188 ms across 30,680 zones. Its
zone-filtered native samples most often resolved to Rumoca's compiled
assignment schedule (821 inclusive stack hits), allocator zeroing (451), and
compiled Jacobian rows (398). The Modelica spans remain scoped candidates; this
capture establishes no before/after gain for them. The paired result for the
Rhai map conversion is recorded below.

### Paired Tracy result — Rhai large-map conversion — 2026-11-08

The large-map conversion now collects directly into Rhai's native `BTreeMap`.
Four interleaved production captures used the same Griffin
`OpenFile` scene, High quality, 2560×1440 target, 1,000 Hz Tracy sampling, and
`landing_handoff` qualification. API / Tracy ports were 4257 / 8207 and
4260 / 8210 for the baseline; 4258 / 8208 and 4259 / 8209 for the candidate.
Each trace's final ten seconds contained 398 complete
`exposure_runtime_surface_projection` zones.

The paired baseline means / p95 values were 1.129 / 2.366 ms and 1.242 / 2.529
ms. Candidate values were 1.058 / 2.187 ms and 1.035 / 2.199 ms. Exact-zone CPU
samples fell from 453 / 490 to 416 / 412 across the two pairs. Native samples
resolved against the run ELF show lower `hook_to_dynamic` and allocator-zeroing
activity with the direct collection path. The existing nested-map conversion
test confirms duplicate-key, scalar and container behavior.

All four runs passed `landing_handoff`, reported zero runtime diagnostics, and
released their app and Tracy ports. The receipts and compact exports are under
`target/perf/tracy-20261108/repeat-abba-*`. This is Tracy-only zone evidence; it
does not establish an FPS result. The 150 FPS objective remains open.

### Qualified opaque-pass diagnostic — 2026-11-09

The release Tracy binary (`-C strip=none`) has SHA256
`8527af155fc204050bfb5dcf058dcaa5b7c764b3364d0c6c15bd1ffc1e072b14`, built
from application HEAD `5ae1388` with working-diff SHA256
`bbb9ab6a83941feebbbc71669bfc8a056b6b869bf294b6c21d3d4ce052cec397`. It uses
the pinned LunCoSim Bevy revision `8671dfe` and Twin HEAD
`b97c679581599195940833a25191d3cbaaa6874a`. The exact Griffin `OpenFile` scene
ran at High / 2560×1440 on API 4279 with Tracy 0.14.1 on 8219 at 1 kHz. The
scene became ready at 13.81 s; `landing_handoff` qualified at 41.80 s. The
admitted Twin source hashes stayed unchanged, final structured diagnostics had
zero errors and warnings, and both ports and the owned process were released.

The 56.24-second capture contains 12,993,107 zones. Its 378,316,017-byte file
contains 378,220,955 compressed payload bytes, a six-byte header, and 23,764
four-byte block lengths; the payload matches the collector's 360.70 MiB report.
The matching Tracy 0.14.1 CSV exporter parsed the GPU events successfully. In
trace time 42.0–56.24 s, the native GPU export reports these mean / p95 values:

| GPU zone | Mean / p95 (ms) | Events |
| --- | ---: | ---: |
| `lunco_shadow_camera_early` | 3.357 / 3.550 | 641 |
| Nested `per_view_shadow_pass` | 1.764 / 1.864 | 641 |
| `main_opaque_pass_3d` | 2.767 / 3.020 | 641 |
| `early prepass` | 0.628 / 0.656 | 641 |
| `bloom` | 0.260 / 0.272 | 641 |
| `main_transparent_pass_3d` | 0.044 / 0.078 | 641 |
| Nested `procedural_skybox` | 0.00028 / 0.00051 | 12 |

The new app-owned `procedural_skybox` GPU marker shows that the sky draw is
negligible for this downward-facing landing view. The opaque pass remains a
useful investigation target, but this capture does not attribute its cost to a
specific material or draw. The qualified workload snapshot also contains the
separate simulator on API 4199 at about 12.4% CPU, so this is diagnostic data,
not an uncontended before/after comparison. Runtime logs report an unresolved
`vehicles/flip_v5.usda` reference. The two-sun warning fires during projection;
51.8 ms later the log marks `/Griffin1SurfaceOps/SolarSystem/Earth/Earthshine`
as a body-fill light. Since that authored fill disables shadows, this startup
warning does not prove a second steady-state shadow caster. The capture, native GPU
export, receipt and images are under
`target/perf/tracy-20261109/opaque-custom-4279/`. No FPS result is inferred.

### Terrain shader A/B/A — 2026-11-09

Three matched Tracy captures isolate the current `lunar_brdf.wesl`,
`terrain_layered.wesl`, and `terrain_surface.wesl` edits. Candidate A1 is the
4279 capture above; baseline B used the committed versions of just those three
shader files on API 4280 / Tracy 8220; candidate A2 restored the working files
on API 4281 / Tracy 8221. All used the same release ELF, Bevy revision, exact
Griffin `OpenFile` scene, High / 2560×1440, Tracy 0.14.1 at 1 kHz, and
`landing_handoff`. Each run preserved the admitted Twin source hashes, passed
with zero structured diagnostics, and released its process and ports. The
candidate and baseline shader hashes are recorded in
`target/perf/tracy-20261109/terrain-shader-abba/`. The qualification times were
41.803 s, 41.677 s, and 41.303 s for A1, B, and A2. All three trace files pass
header/block-length framing checks, and all three matching native CSV exports
completed successfully.

The common 43–53 s trace window gives these GPU-zone means / p95 values:

| GPU zone (ms) | Candidate A1 | Baseline B | Candidate A2 |
| --- | ---: | ---: | ---: |
| `main_opaque_pass_3d` | 2.746 / 2.882 | 2.758 / 2.859 | 2.759 / 2.907 |
| `lunco_shadow_camera_early` | 3.372 / 3.553 | 3.310 / 3.545 | 3.382 / 3.553 |
| Nested `per_view_shadow_pass` | 1.772 / 1.864 | 1.742 / 1.865 | 1.779 / 1.870 |

The two candidate opaque-pass means average 2.753 ms versus 2.758 ms for the
baseline (0.2% lower); the candidate p95 is higher, and A2 does not repeat A1's
small mean difference. Treat this as no demonstrated GPU gain. Final viewport
images look the same and retain the lander and long shadow; first-ready images
vary more while scene assets stream. The qualified snapshots show the separate
4199 simulator at 12.1–12.4% CPU in all three runs. Captures and native GPU
exports are under `target/perf/tracy-20261109/terrain-shader-abba/` and
`target/perf/tracy-20261109/opaque-custom-4279/`. This shader bracket does not
establish an FPS result or explain the opaque pass's per-material cost.

### Opaque draw-category Tracy diagnosis — 2026-11-09

To split the opaque GPU pass, the local Bevy diagnostic added timestamp spans
around batchable/multidrawable meshes, unbatchable meshes, and non-mesh items.
The app was built in release mode with Tracy and symbols (`-C strip=none`), using
the stable 0.20 fork worktree at `8671dfe` for the current simulator dependency
cohort. The binary SHA256 is
`8e697d4f8dd7e4e68ee22adea2cc864ad3a1dc355cce0097d97e4f94497b65aa`.

The exact Griffin `OpenFile` scene ran at High / 2560×1440 on API 4340 with
Tracy 0.14.1 on 8340 at 1 kHz. `landing_handoff` qualified; source hashes
matched before and after; final diagnostics were 0 errors / 0 warnings; first
ready and final images were captured and inspected. Both owned ports and the
app process were released. The 69.4-second capture contains 10,264,789 zones;
the 385,242,552-byte trace and native GPU export are under
`target/perf/opaque-phase-griffin-20261109/`. The run used application HEAD
`5ae1388`, working-diff SHA256
`70c5d2d86b8aaa76df5e6a7c361ece9ba9feb1198592dfe053a01e3362f3f868`, and Twin
HEAD `b97c679581599195940833a25191d3cbaaa6874a`.

The 58.0–68.3 s tail contains 287 resolved main-camera pass samples. Values are
mean / p50 / p95 GPU milliseconds:

| GPU zone | Mean / p50 / p95 (ms) | Events |
| --- | ---: | ---: |
| `main_opaque_pass_3d` | 2.958 / 2.697 / 4.550 | 287 |
| `main_opaque_pass_3d/opaque_phase` | 2.956 / 2.694 / 4.548 | 287 |
| `.../opaque_phase/batchable_and_multidrawable_meshes` | 2.956 / 2.694 / 4.548 | 287 |
| `.../opaque_phase/unbatchable_meshes` | 0.0003 / 0.0003 / 0.0003 | 2 |
| `.../opaque_phase/non_mesh_items` | 0.0003 / 0.0003 / 0.0003 | 7 |
| `lunco_shadow_camera_early` | 4.137 / 3.530 / 5.530 | 287 |
| Nested `per_view_shadow_pass` | 2.109 / 1.854 / 3.676 | 287 |
| `early prepass` | 0.593 / 0.593 / 0.595 | 287 |

The opaque phase's resolved GPU time is almost entirely in batchable and
multidrawable mesh work; unbatchable and non-mesh records are too sparse to
support per-frame estimates, and the alpha-mask phase was empty. This narrows
the next investigation to the visible mesh/material workload; it does not yet
attribute cost to a particular material or shader. The matching candidate,
baseline, candidate shader A/B/A found no measurable terrain-shader gain.

This was a diagnostic capture with another LunCoSim session (API 4155) and
`lunica` (API 4187) active. The elevated p95 values and concurrent GPU workload
make it unsuitable as an A/B comparison. Do not infer FPS from these Tracy
zones.

The fork branch `perf/opaque-phase-tracy-diagnostics` is now rebased locally on
upstream Bevy `main` at `278a32f65e0fbd937551c0573050c25f587c57f8`; its receiver
culling commit is `eb8f4bb67`. The receiver-culling code and nested timestamp
markers passed `cargo +1.97.1 check -p bevy_core_pipeline -p bevy_light -j 4`
in a separate worktree with the matching main workspace. The simulator capture
still used the stable 0.20 dependency cohort at `8671dfe`; it does not establish
runtime behavior on latest main. No Bevy branch was pushed.

## Rumoca current-main integration — 2026-11-22

LunCoSim's Rumoca family is pinned to upstream commit
`488d6d5f239fcae2bf4ce5a1b729a23d2cc6c396` from CogniPilot in `Cargo.toml`
and `Cargo.lock`.
Adaptive and fixed-step sessions use Rumoca's public `SimulationSession`;
LunCoSim selects `SimSolverMode::FixedStep` for its registered `fixed-rk4`
capability. Requested-value selection uses Rumoca's
`SimulationSession::selected_values`. The application retains checked
compiled-DAE caching and uses Rumoca's default parallel Cranelift compile queue.
It does not install Rumoca's optional native-code artifact store: that scope
currently compiles queued native functions serially, and a second
native-artifact persistence path would add a large owner without helping
steady-state stepping.

The earlier LunCoSim fork-pinned baseline
(`e86e9a0137562fca38998524fc5ac2a4600b41f1`) had these focused checks:

- `cargo test --offline --locked -p lunco-modelica-solver --test simulation_session -j 4` — 2 passed.
- `cargo test --offline --locked -p lunco-modelica-worker --test rumoca_api_coverage -j 4` — 3 passed.
- `cargo build --offline --locked -p lunco-luncosim --bin luncosim -j 4` — passed. Binary SHA-256: `82012370ba76eb84ba17549ece002cde347124082fed53ec96fe7e3cef7159df`.

That baseline's production headless startup harness opened the exact scene
`/home/rod/Documents/models/lunar-base-model/twins/astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda`
from Twin commit `8efcaab022a41e1dd88be078cb94bcc06950865c`. On API port 4631,
it reached first physics admission in 158.81 s. `/Griffin1SurfaceOps/Lander`
was ready and unpaused; 13 Modelica participants were running, with zero
runtime errors or warnings. The powered-descent scenario started, the source
snapshot was unchanged, and the owned process and port were released. The
receipt and log are in
`target/perf/rumoca-main-e86/griffin-headless/`.

At first admission, `AttitudePropulsion` reported a 1.497 s session
construction, 15.52 ms worker step, 21.04 ms response latency, and backlog 8.
These are first-step figures, not a settled physics or frame-time sample. The
headless run reports no rendered FPS. This is a prior-build startup baseline,
not validation of the upstream Rumoca pin below.

### Upstream Rumoca build and Griffin startup — 2026-11-22

The source-checkout debug build used Rumoca upstream commit
`488d6d5f239fcae2bf4ce5a1b729a23d2cc6c396`. LunCoSim application HEAD was
`c6967cb615bdddf05cc5e6c51326f5baf2136d51` with the working diff recorded in
the receipt; the binary SHA-256 was
`fef8ba89d336bf6cf773ff01333bcb433f90257524ede53ce54f9c4e314de59b`.

`scripts/perf/twin_startup.py --startup-only --headless` opened the exact
`scenes/griffin_1_surface_ops.usda` scene through `OpenFile` on API port 5417.
The Twin was still at `8efcaab022a41e1dd88be078cb94bcc06950865c`; its 594-file
source snapshot stayed unchanged. Application bootstrap reached readiness in
1.302 s, then the scene reached first physics admission in 38.309 s. The
lander was ready, 13 Modelica participants were running and two were idle, and
`RuntimeDiagnostics` reported zero errors and warnings. The process and API
port were released after the run.

The log recorded one coupled initialization for eight participants and no
`reset=true` lifecycle result. The retained RK45 coupled-initialization
observation profile was 20.601 s; compiling its generated DAE took 2.685 s on
the compiler actor. This points to coupled initialization as the main
remaining load cost. The previous 158.81 s fork-pinned startup is about 4.1×
longer, but its receipt and workload/cache snapshot are no longer present, so
this is not a controlled version-only comparison.

Headless `EnginePerformance` had no frame samples, so this run does not provide
rendered FPS. A clean windowed run was unavailable: this environment has no
private Xvfb and cannot access the NVIDIA driver; the user's display was left
untouched. The receipt and log are in
`target/perf/rumoca-main-current/griffin-headless-20261122/`.

After integrating the newer local `main`, the combined tree passed
`cargo test --locked -p lunco-modelica-solver --test simulation_session -j 4`
(3 passed) and `cargo build --locked -p lunco-luncosim --bin luncosim -j 4`.

## Griffin surface-present attribution — 2026-11-21

A matching Tracy 0.14.1 profile used the instrumented production binary
`6b85addae67a7a3dbca44e6773bf7f9cc39444ac4b353ed5b94128e51f97753e`,
1 kHz CPU sampling, the 1600×1000 private Xvfb display and the RTX 5060 Laptop
GPU. It opened the exact `griffin_1_surface_ops.usda` scene on the verified
lander body. Readiness stayed clear and the lander stayed unpaused throughout
the 12-second zone window from 138 to 150 seconds. The full harness later
rejected the run: the instrumented process had advanced only 134 fixed ticks
after qualification when a terrain stream item became pending, before its
240-tick gate. It is diagnostic evidence, not FPS
acceptance. The receipt and exports are in
`target/perf/rumoca-main-cutover/tracy-settled-gui-corrected/`.

Across 131 main-app frames, the CPU `main app` span averaged 88.82 ms (p50
86.24, p95 119.78). Its `service_simulation_during_render` system averaged
64.93 ms (p50 64.56, p95 86.24). On the render thread, Bevy's `present_frames`
span averaged 56.91 ms (p50 55.40, p95 67.85), while the `RenderGraph` schedule
averaged 8.69 ms. Bevy's `present_frames` span wraps the window present call
after command submission. LunCoSim's render-time host waits for the render app
to return its frame. The measured frame delay therefore sits at surface
presentation, not in Rumoca compilation or the physics solve.

GPU timestamps show the dominant individual passes around 0.76–2.03 ms per
frame; physics-step service in the sampled health history was 2.66–6.94 ms.
Modelica worker CPU remains substantial when summed across 15 participants,
but those aggregate worker milliseconds are not the frame's critical-path
duration. The startup log also reports `environment_sun_light_projection`
firing on 300/300 gate evaluations; that is a separate ineffective change
gate, not the source of the measured 56.91 ms present span.

The run requested `--no-vsync`, which selects WGPU `AutoNoVsync`, but the
runtime receipt does not record the surface mode selected by the backend. The
capture localizes the stall to `window.present`; it does not distinguish an
unsupported-mode fallback from a driver or display-server present wait. The
instrumented private-display result also does not establish the exact cause of
the original desktop 40 ms report. A matching desktop capture with the
negotiated surface mode recorded is still needed for that distinction.

## Acceptance still required

The current graphics and simulation targets remain 150 FPS and sub-0.5 ms
physics. Acceptance requires an uncontended production run at the authored
quality, complete scene readiness, unchanged source cohort, a settled frame
window, and separate physics/service timings. Profiling runs are diagnostic and
do not supply the product FPS number.


## Griffin follow-up — 2026-11-25

The official-fork revisions used for the following measurements prepare Rumoca exact assignment certificates
and indexed projection diagnostics (`9763caab5014875da353663df14c75fe0cd8e990`),
and return Bevy's render world before its bounded native presentation batch
(`33c9a2d62c5381b2e4ae72f8ebe189928b3127f2`). Both commits were pinned for these captures. Application changes balance persistent solver lanes by admitted scalar
weight, reuse ordered observation names and parameter lookups, retain optical
geometry and receiver-domain proofs, avoid equal-value sun light invalidation
and skip under-budget terrain eviction sorting. The complete runtime observation
contract, 60 Hz fixed quantum and High rendering quality remain unchanged.

The first non-Tracy 1600×1000 private-X11 release comparison was 11.13 versus
11.28 FPS, p95 117.63 versus 98.30 ms. A second back-to-back preserved-binary
replay on the same private surface measured:

| Metric | Baseline | Candidate |
|---|---:|---:|
| Running mean FPS | 9.66 | 12.51 |
| Running p95 frame ms | 164.91 | 91.96 |
| Paused mean FPS | 12.69 | 12.85 |
| Observed fixed ticks/wall s | 12.90 | 11.94 |
| MainPropulsion mean worker ms | 82.45 | 83.44 |

Both replayed the Griffin touchdown/handoff at 22.516667 simulation seconds,
with identical 597-file source inventories, 193/193 terrain tiles and zero
final runtime diagnostic findings. Pause held the authoritative tick for the
whole sample. Host contention differed (the baseline overlapped a large
linker job), so the observed delta is not an isolated performance claim. The
private surface selected Immediate for the Mailbox request; only FIFO and
Immediate were supported. Native desktop and 2K acceptance remain open.

Focused fork tests, ordered observation and affinity tests, receiver-domain
and exact light-value regressions passed. Production terrain visibility
(including negative admission), initialization/receipt, negative Modelica
observation and UI preparation/scene-contract verdicts passed. The headless
dependency gate confirmed absence of the GPU stack but found the existing
input-focus dependency from `lunco-control-core`.

The paused Xvfb FPS ceiling and unchanged MainPropulsion mean establish that the
performance objective is not achieved. A bounded render-world handoff does
not isolate the authoritative clock; independent immutable presentation and
reliable render extraction lifecycle are still required. Remaining solver
work is batched execution of the actual hot scalar blocks with exact replay
evidence, subscriber-specific observation admission, static UI policy versus
continuous binding publication, active-viewport picking invalidation and
static/dynamic caster indexing. The precise sun gate still admitted 300/300
live Griffin evaluations; its regression is a correctness improvement, not
proof of a scene-level reduction. Do not relax simulation or render fidelity
to reach the target.

## Native Griffin presentation on Linux

The same High-quality release executable and 1600×1000 Griffin tracking camera
ran at 21.84 FPS on Xvfb and 90.23 FPS on a private Weston GL output. Paused
rates were 21.62 and 109.20 FPS. Both used NVIDIA Vulkan on the RTX 5060.
Xvfb offered FIFO and Immediate and selected Immediate. Wayland also offered
Mailbox and selected it. Weston reported the NVIDIA DRM render node and DMA-BUF
modifier support. Runtime receipts retained all 597 source hashes, 193 resident
terrain tiles with none pending, and the authored landing handoff at 22.516667 s.

In a separate pair of runs with identical instrumented executable hashes,
Tracy attributed 43.89 ms per present call to Xvfb and 4.87 ms to Wayland while
running. Respective p95s were 47.28 and 6.34 ms. Acquire averaged 0.024 and
0.027 ms. A standalone NVIDIA Vulkan cube requested Immediate on both outputs.
The whole 240-frame processes took 8.48 s on Xvfb and 0.35 s on Wayland.
This reproduces the display-path penalty independently of LunCoSim and of the
choice between Mailbox and Immediate. The measurements do not identify the
operation inside the proprietary driver responsible for the difference.

At 2560×1440, the private GPU Wayland output delivered 82.18 application FPS
with a 15.67 ms p95. The active camera retained its requested physical target
throughout the window. The renderer's bounded batch still owns the texture,
native window and queue until presentation completes. Adding another queued
frame would change latency without eliminating the measured transport cost.
Use the [native profiling driver](../../scripts/perf/README.md) for repeatable
surface comparisons. Product acceptance on a monitor requires its own desktop
cohort. Receipts and matched traces are under `target/perf/griffin-native-wsi/`
and `target/perf/griffin-repeat-work-fixed-20261125/`.

Two consecutive release runs at 1600×1000 used the same NVIDIA Vulkan client.
With the Radeon 610M compositing the window, Griffin ran at 92.71 FPS
(p95 13.96 ms). With the RTX 5060 compositing, it ran at 95.81 FPS
(p95 13.67 ms). One-second process samples detected no compiler CPU time in
either run. The earlier 42.58 FPS hybrid run overlapped three compilers.

In the matched Tracy captures, the hybrid render thread spent 5.57 ms per
native present call while running. Only 0.11 ms on average remained after
the next extraction finished. The same-GPU figures were 4.87 ms and 0.02 ms.
All 1,002 hybrid and 978 same-GPU running presents matched the adjacent
render updates and extractions. With physics paused, 4.22–4.38 ms remained
after extraction, and the renderer performed 0.72–0.73 ms of work before
acquiring the next image. These intervals describe the existing schedule.

With ordinary focused-window pacing and FIFO, the private Weston output
gave 39.71 application FPS. A diagnostic Mailbox run recorded 95.18
application FPS and 39.58 completed output repaints per second in the same
settled window. On private Mutter outputs configured at 60 and 180 Hz, the
same FIFO application reported 89.75 and 90.42 FPS. A virtual output's
configured refresh rate alone does not establish its delivery cadence.

In a separate Weston FIFO diagnostic, Griffin produced 39.62 application
frames per second and the virtual output completed 39.61 repaints per second.
Wayland registered three 1600×1000 DMA-BUFs for the window at startup. From
landing handoff through exit, each buffer was attached 276 times and the
window imported no additional full-size buffer. Keep the bounded render
handoff. Across the paired running frames, moving existing pre-acquire work
alongside the outstanding present could overlap at most 0.046 ms per frame
on average on the hybrid path and 0.013 ms on the same-GPU path.

## Griffin main propulsion on upstream Rumoca e0a856fea

Replaying the first 1,440 production input transactions at 60 Hz through
LunCoSim’s release solver adapter produced identical binary64 values across
all 3,295 observations with the prior fork and the upstream merge. Both took
2,711 internal steps in the first 1,352 communication steps and another 176
in the final 88. In the latter window each made 5,686,320 singleton visits.

At this pin the ordering pass stopped at the first compact declaration run.
Griffin contained 2,662 explicit stages and 14 such runs. The first rejected
dependency was a plume calculation at scalar block 119 reading its observation
length from block 620.

The four sequential release replays ran on logical CPU 12. Each entry is a
mean per 1/60-second communication step in milliseconds. Full step includes
input admission, integration and reading all observations.

| Run order | Flight integration | Flight full step | Settled integration | Settled full step | Host load average, start to end |
|---|---:|---:|---:|---:|---:|
| Prior pin, first | 48.47 | 66.14 | 22.79 | 25.73 | 42.61 to 19.01 |
| Upstream merge, first | 26.25 | 32.73 | 15.39 | 16.98 | 19.01 to 10.15 |
| Upstream merge, second | 22.67 | 27.90 | 15.38 | 17.10 | 10.15 to 6.24 |
| Prior pin, second | 24.43 | 30.36 | 15.97 | 17.64 | 6.24 to 3.77 |

The last two runs give the closer comparison under lower host load. In every
run the ordered observation stream had SHA-256
`f9b17c3d55fba129cb4848af93776d569b3c75ec2f2d834b034cfb16bee89d9b`.
The production engine-out, fuel-exhaustion and oxidizer-exhaustion scenes
passed at ticks 242, 213 and 125 respectively.

The updated 1600×1000 High, private-Wayland mission reached landing handoff.
It averaged 49.41 application FPS, 15.83 authoritative ticks/s and 45.11 ms
per settled MainPropulsion worker step. Other simulator activity consumed
72.86 sampled CPU-seconds in the measured window. The earlier qualified
baseline averaged 77.11 FPS and 26.13 ticks/s with 2.7 other-simulator
CPU-seconds. The fresh headful baseline lost its build and capture directory
after scene readiness. The different contention levels prevent attributing
the frame-rate difference to Rumoca.

## Griffin main propulsion after mixed projection ordering

Rumoca `2a3716da596e18598e21a3683b1ff6d13ee650a0` orders compact
declaration runs alongside explicit blocks. A run moves as a unit. An explicit
assignment reads its own output dependencies and the obligations executed
by the source program. This distinction matters for the seven nozzle walls.
Treating every array output as a reader of all sibling values creates a false
cycle between the area fractions and zone radiation. The issued schedule now
passes Rumoca's execution-order check for the algebraic, derivative, root and
event refreshes. Each has 15 stages. The seven coupled valve and chamber
blocks still perform their numerical solves.

The comparison used the same checked DAE, 60 Hz communication points,
1/180 s configured integration step, RK solver and `1e-6` tolerances. The
baseline production Twin supplied all ten inputs at each of 1,440 consecutive
steps from mission start through 24 s. Replaying that capture with the old
release solver reproduced the previous full observation-stream SHA-256.
Both solver versions took 2,711 internal steps during flight and 176 in the
settled window.

| Recorded descent work | Previous | Updated |
|---|---:|---:|
| Single-row projection calls, first 1,352 steps | 127,105,016 | 0 |
| Single-row projection calls, final 88 steps | 5,686,320 | 0 |
| Coupled-block calls, first 1,352 steps | 1,375,094 | 473,816 |
| Coupled-block calls, final 88 steps | 56,287 | 29,071 |

All 4,744,800 sampled binary64 values were numerically equal. Exactly 20,566
zero values across 91 channels changed sign bit. Every nonzero value was
bit-identical. A separate 24 s test exercised a throttle ramp, an engine
shutdown and restart, RCS consumption and commanded cutoff. All values
matched there too, with 91 signed-zero differences at the first step. Both
tests included every public observation and retained the authored plume
photometry and transport, wall thermal dynamics, tank depletion and thrust.

The full Rumoca evaluator suite passed all 620 tests. The production Rhai
engine-out, fuel-exhaustion and oxidizer-exhaustion scenarios passed at ticks
242, 213 and 125, matching the previous solver.

The final release replays ran consecutively on logical CPU 12, after the
desktop session exited. A full step includes input admission, integration,
and reading all 3,295 observations. The phase CPU measurement uses the Linux
process user and system counters between solver initialization and the last
step. The host one-minute load averaged about 29 at the start of the baseline
flight and 18 at the start of the updated flight.

| Same 24 s descent | Previous | Updated |
|---|---:|---:|
| Mean full step in flight | 69.94 ms | 6.44 ms |
| Mean integration in flight | 52.53 ms | 4.64 ms |
| Mean full step after touchdown | 29.58 ms | 3.62 ms |
| Mean integration after touchdown | 25.81 ms | 3.10 ms |
| Replay process CPU time | 96.97 s | 9.09 s |

The updated solver repeated at 7.49 and 9.41 ms per full flight step in two
earlier runs under build load. The prior pin has also measured 27.90 ms in
flight on a quieter host. The projection call counts and numerical comparisons
were repeatable across runs.

Separate full-Twin headless runs recorded identical ten-input maps, vehicle
mass and engine-one thrust for every accepted step. At 24 s the vehicle
mass was 3,936.629725705765 kg. During concurrent builds and runtime tests,
physics admission took 56.75 s with the prior pin and 209.99 s with the new
pin. Those runs establish live behavior. Isolated solver preparation,
including the benchmark's separate plan inspection, used 9.52 and 8.46
process CPU-seconds respectively.

The integrated native release ran on the RTX 5060 Laptop GPU using Vulkan,
a private Wayland output, 1600×1000 High quality and the authored tracking
camera. The rendered lander reached touchdown. The settled window recorded
480 frames at 31.54 mean application FPS, with 30.95 ms median and 47.47 ms
p95 frame times. Physics advanced at 26.48 ticks/s. MainPropulsion averaged
14.09 ms of worker time over 1,997 samples from mission start, and the
runtime reported zero errors. Four concurrent compiler processes appeared
in the workload snapshot at landing. These host conditions differ from the
earlier native measurements, so the FPS change attributable to Rumoca is
unresolved.
