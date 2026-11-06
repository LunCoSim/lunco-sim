# Selected directional-light response qualification — 2026-11-05

## Scope and ownership

The maintained Bevy PBR owner consumes a fragment-local selected-light
response. Application terrain shaders configure it through their shared Sun
owner. The [canonical contract](../architecture/render-decoupling.md#selected-directional-light-response)
describes its API, defaults, lifecycle and production gate. Four application
shader consumers use that contract; their duplicate native lighting reconstruction
is removed. No Modelica equations, physics solver settings, terrain quality,
shadow-map resolution or authored lunar-base-model source is changed.

The workspace pins the shader, render, light and PBR owners to published Bevy
revision `65dd8f4e86383c19128a33618a6a8bfc3427fd3a` on
`fix/gpu-shader-buffer-stride`. Ordinary builds resolve that revision from GitHub
without a sibling Bevy checkout or command-line patch override.

The deferred CCD island/constrained-inertia work is recorded beside native
motion clipping in the published Avian branch `fix/checked-inertia-numerics`
at `0a669ddfc366723af19c7903c240b10cf3bbc3ad`. That comment-only commit does
not change the Avian pin or qualify the inherited articulated CCD failure.

## GPU evidence

Artifacts are under `target/perf/graphics-20261105/`. The production Tracy client
is captured with adjacent Tracy 0.14.1, started before each owned app. Each valid
window reaches simulation tick 1600 through `graphics_fixed_pose.rhai`, pauses,
and retains readiness, camera, terrain, collider, image and source receipts.
The ROI starts six seconds after the native `GRAPHICS_FIXED_POSE` message and
ends two seconds before the last GPU event. These frozen diagnostic timings are
separate from running, uninstrumented product FPS.

| Windowed diagnostic | Opaque mean ms | Directional shadow mean ms |
|---|---:|---:|
| First baseline | 9.3223 | 7.6219 |
| Second baseline | 9.8496 | 7.6610 |
| Compact selected-light response | 9.6022 | 7.6430 |

All windows retain four active Sun cascades with 1,070/1,109/1,150/1,219
casters and 190 terrain meshes. The compact response does not establish a
GPU speedup against this baseline range. An angular-admission trial was flat;
a per-fragment array trial increased opaque cost to 11.3176 ms. Both were
rejected. This experiment record prevents repeating those costly trials; neither
implementation remains in source.

The additional offscreen baseline overlapped `cosmic-player` playback and
thumbnail generation. GPU memory clocks also changed. It is contention-affected
and excluded from performance acceptance; no other session was controlled.

The compact viewport's mean absolute channel difference from the second
baseline is 0.03117 on a 0–255 scale, with 0.1691% of channels differing by more
than one. Repeated baseline variation is 0.03736 and 0.1926%, respectively.
Simulation tick and caster counts match; render origins and very small physical
pose differences prevent a bit-identical image claim. A live shader diagnostic
through `SetShaderSource` confirms actual selected-Sun admission over 96.1591%
of the Griffin image, restores the original source, and retains all 548 guarded
model-source hashes. It is a correctness probe during compilation, not an FPS
measurement.

## Production validation

The qualification build passed `cargo build --release --locked --offline -p
lunco-luncosim -j4` using the then-local PBR owner. The retained executable is
`target/perf/graphics-20261105/luncosim-normal-masked`, SHA256
`0406f0c7b512a3b1547ea2b64430bebdfbd7abb3eb18d54a1048747d3c7bb0b4`.
This retained executable is the qualification artifact for this review.
Subsequent builds replace `target/release/luncosim`; its current contents require
separate provenance.

The production `test_directional_light_response.py` gate passes on port 6052:
four authored Rhai setup checks, both positive GPU colors, no red failure
pixels, zero diagnostic errors, unchanged fixture source and verified owned
Exit. Its shader exercises default identity, native accumulation, independent
fill, selected-Sun response, missing selection and bounded negative/visibility
controls. Native clearcoat/transmission equations are unchanged; no separate
cross-platform or debug-cascade visual qualification is claimed.

The final uninstrumented High/2560×1440 before/candidate/after comparison uses
the same authored scene, landing handoff gate, five-second settled admission,
at least ten seconds of frame history and advancing physics:

| Running window | Mean FPS | Frame p50/p95 ms | Coupled-tick p50 ms |
|---|---:|---:|---:|
| Baseline before | 45.8615 | 20.2118 / 34.9978 | 3.9561 |
| Candidate | 30.6166 | 29.7078 / 55.6238 | 5.5038 |
| Same baseline after | 20.7363 | 43.6182 / 78.7410 | 6.7288 |

The repeated baseline has the identical executable SHA256
`1fa0289a78262b492a23e00e6172edd8b5942d60c392b669fb35f18cc8ca03c1`
and identical shader/model sources. Its large degradation, alongside falling
GPU clocks/utilization and concurrent desktop/browser activity, invalidates a
causal FPS comparison. GPU workload/clocks receipts are retained in each
window. This pass proves runtime admission and advancing healthy simulation,
not a product FPS improvement or regression. The 1 ms Modelica and 150 FPS
targets are not established. The local refactor is retained for one native
lighting owner and source deduplication; no graphics speedup is claimed.

All three running windows preserve the same 548 source hashes and report zero
runtime diagnostic errors. All owned apps and collectors have exited; API and
Tracy listening ports are released. Only the touched Rust file was formatted,
after validation; complete application, Bevy and Avian diffs pass whitespace
checks. No new dependency patch or application change was pushed.
