# Directional receiver raster qualification — 2026-11-06

## Result and retained owner

High/2560×1440 lunar-base-model Griffin improves from **51.5620 to 55.3419 FPS
(+7.33%)** in an alternating, uninstrumented, same-process comparison. Pooled
frame median improves from **19.3899 to 18.0432 ms** and p95 from **20.2382 to
18.9924 ms**. These are short settled windows, not a sustained 150 FPS result.
One cropped-frame outlier reaches 51.4970 ms; worst-case latency is not solved.

`lunco-render-bevy` scissor-bounds native directional shadow draws to their
possible receiver domain. The
[canonical contract](../architecture/render-decoupling.md#directional-shadow-receiver-raster-bounds)
covers source qualification, native projection/filter/bias envelopes, owner and
lifetime. Registration uses typed resources, preserving parallel preparation.
Native phase preparation resets each delegate's applied-view state, so it emits
scissor state once per delegate/view rather than once per mesh draw. All native
draw IDs and commands remain owned by Bevy. This application integration uses
public APIs; no new dependency Rust patch is required.

The atlas remains four 8192² cascades. Casters, LOD, materials, lighting,
filtering, antialiasing, equations and simulation settings are unchanged. Eight
remains the default physics substep count. There are no authored USD edits.
Four Griffin Twin finish shaders add only the existing reflection capability
annotation; their shading equations and detail are unchanged. Other pre-existing
model/media and application work is preserved. Continuous raster math and safe
admission are renderer invariants; no scenario policy or Twin identity is
hardcoded in Rust. Rhai owns the test controls and assertions.

## Product FPS acceptance

Evidence: `target/perf/fps-20261105/within-session-phase/`. The owned normal
process uses API 6128, High, a verified 2560×1440 native window and pointer
passthrough. `graphics_fixed_pose.rhai` first pauses at exact tick 1600. Native
zero `TemporalJitter` requests conservative full raster; removing it restores
ordinary bounded raster. Full/cropped/full paused scene viewports are
**byte-identical** across 2560×1360 pixels, excluding application bars. Source
shaders are never edited during this comparison.

Physics is resumed before FPS sampling. Each mode settles for three seconds,
then collects ten seconds of contiguous native frame history. The authored
`shadow_receiver_crop.rhai` predicate helper checks admission without sending
notification toasts during the measurements. An authored four-check verdict
runs after the final window. All windows use the same loaded scene, pipeline,
GPU allocation and live process.

| Window | Raster | Frames | Mean FPS | Frame p50 / p95 ms |
|---|---|---:|---:|---:|
| A0 | Full | 513 | 51.8864 | 19.2682 / 19.9393 |
| B0 | Cropped | 538 | 55.4136 | 17.9994 / 18.9097 |
| A1 | Full | 502 | 51.5913 | 19.4140 / 20.0131 |
| B1 | Cropped | 536 | 55.2702 | 18.1070 / 19.0262 |
| A2 | Full | 500 | 51.2044 | 19.4955 / 20.8293 |

The pooled result uses every frame: 1,515 full and 1,074 cropped, with no
outlier removal. Each cropped window improves over its two neighboring full
windows by **7.10% / 7.53%**. Memory clocks remain 11,001 MHz; phase mean SM
clocks drift from 2,638 to 2,575 MHz as the GPU warms from 80 to 86 C. Cropped
windows do not benefit from a higher GPU clock. The monitor and workload
receipts are retained; no compiler overlaps this acceptance run.

Clock contracts remain valid with transport/virtual rates both 1.0, active
physics and no physics holds. Simulation advances from tick 1840 to 5864 over
67.09 wall seconds, retaining 60 Hz physics and eight substeps (16.666667 ms
step / 2.083333 ms substep). Each measured window advances 611–622 ticks.
Coupled service medians remain 3.117–3.242 ms; this is not a Modelica worker
service-attribution measurement or a 1 ms solver claim. Body state stays ready
and unpaused. Diagnostics have zero errors/warnings and all 548 admitted model
source hashes match afterward. The owned app exits and its port is released.

## Native GPU evidence

`receiver-crop/` uses a temporary Rhai control in one paused diagnostic process,
full/cropped/full/cropped/full at tick 1600. That probe control/environment flag
is absent from production source. Tracy 0.14.1 capture starts before the app;
native GPU/messages exports succeed. The ROI removes two seconds at both ends
of each ten-second window.

| Diagnostic window | Shadow mean ms | Opaque mean ms |
|---|---:|---:|
| Full A0 | 7.6714 | 8.5571 |
| Cropped B0 | 6.3020 | 8.0490 |
| Full A1 | 7.6881 | 8.0408 |
| Cropped B1 | 6.3132 | 8.2316 |
| Full A2 | 7.7122 | 8.2850 |

Shadow time falls about **1.38 ms / 18%**. No opaque-pass gain is attributed.
All five scene viewports are byte-identical; diagnostics/source guards pass and
owned ports 6090/8214 close. `receiver-qualified-source/` qualifies the stronger
production bound envelope: 113,207,456 of 268,435,456 candidate texels across
four cascades and 6.2853 ms native shadow time. Preparation overlaps a focused
test build; it is consumer/GPU evidence, not product FPS acceptance. Its 548
source hashes match and owned ports 6096/8218 close. These diagnostic builds
precede final per-phase command coalescing; the product FPS above uses the
final normal executable.

## Production gates and provenance

Final executable: `target/perf/fps-20261105/luncosim-receiver-phase-normal`,
SHA256 `0097be19eb6bd250adcebdfd7d99fad0c5a4140e0db72ecfe3e3089f4493acdc`.
`target/release/luncosim` has the same SHA256. The direct toolchain 1.97.1
Cargo/Rust compiler build uses regular sccache, the normal target directory and
`--release --locked --offline -p lunco-luncosim -j 4`, without Tracy/watcher
features. Build/source receipts identify the maintained local PBR owner and
its shader-only patch, recorded locally as `65dd8f4`. The four Twin finish
annotations are on model main at `44830a7`; the comment-only Avian CCD follow-up
is local commit `0a669dd` and does not change the published behavioral pin.

- `test_shadow_receiver_crop.py`, API 6130: **24 authored checks** across native
  prepass variants, bounded admission, native zero-jitter rejection/removal,
  unknown shader source, source restoration and `ClearScene` retirement.
  Green/cyan each cover 0.00114149 of the image, red is zero, and full/cropped
  image difference is exactly zero. Diagnostics and source guards pass; owned
  Exit releases the port. Evidence: `receiver-delivery-qualification/`.
- Native shared-shadow fixture, API 6126: **four authored checks**, both point
  and spot GPU probes (each 0.00173611 coverage), no red pixels, clear
  diagnostics, matching fixture source and verified owned Exit. Evidence:
  `shared-shadow-crop-phase/`.
- Focused release math test
  `shadow_receiver_crop::tests::rectangle_contains_native_bias_and_filter_support`:
  passes for native f32 lookup arithmetic, both admitted filter radii, bias,
  rotations, million-unit translations, invalid bounds and non-finite matrices.
  It uses the authoritative `bevy::math::proj::orthographic` without a new warning.
- Skill catalogue validates all 43 skills. Final formatting is restricted to
  the two touched renderer Rust files; application, Bevy, model and Avian diffs
  pass whitespace review. The application and companion changes are committed locally for main integration;
  no remote push is performed.

The aggregate render-scene runner remains blocked by pre-existing all-scene
discovery of intentionally invalid `simulation_core_mass_units_invalid.usda`.
The direct production fixture verdicts above are separate evidence, not an
aggregate-suite green claim. The deferred articulated CCD failure is unchanged.

## Rejected trials and comparison limits

Byte-exact vertex deduplication plus near-first native CPU bins produces less
than one percent mean FPS change and worse tails; the Rust trial/tests/docs are
removed. Zero-Fresnel dispatch, Schlick multiplication and equivalent roughness
or half-angle rewrites establish no GPU gain. Removing noise saves roughly a
millisecond but removes detail and is rejected. A standalone four-map 8192²
hierarchy reduction costs roughly 3.81 ms; High uses Hardware2x2 comparison,
while Balanced selects Gaussian. None of those trial implementations remains.

The follow-up `target/perf/fps-20261106/material-detail/` assay uses the
qualified diagnostic executable, paused at tick 1600. Transient shader-source
changes conservatively retire receiver admission, so all five windows use full
native shadow raster (7.636–7.659 ms). Original opaque means are
8.446 / 8.340 / 8.759 ms. Disabling the three optional detail-texture branches
produces byte-identical scene pixels but takes 9.227 ms; sharing their stochastic
sample coordinates takes 9.509 ms, also with identical pixels. Neither trial
establishes a gain and neither is retained. The green shader sentinel covers
97.73% of the image, proving the edited terrain shader is the live consumer.
All 548 model source hashes match, runtime diagnostics are clear and owned
API/Tracy ports 6140/8220 close. This is GPU attribution, not new normal FPS
acceptance; it does not justify removing authored detail or adding material
specialization machinery.

The `fps-20261106/shadow-reuse/` and `shadow-reuse-raw/` captures inspect
native cascade inputs once per second in the fixed Griffin view. Four paused
samples have unchanged matrices, membership, pose and mesh fingerprints. In
20 playing samples, all four cascade matrices and pose fingerprints change in
every one of 19 intervals; membership and mesh fingerprints stay constant.
World-space changes alone are not a reuse verdict because the scene rebases.
For the first 64 candidates per cascade, composing the captured f32 light and
caster matrices in f64 still shows movement: the median per-interval shift of
their local origins is approximately 0.298, 0.080, 0.038 and 0.007 shadow
texels for cascades 0–3, respectively. This is an estimate at sampled origins,
not a bound on all geometry or proof of per-frame changes. A cache must account
for light-space movement, deformation, materials, raster state and allocation;
unchanged membership alone cannot validate reuse. Both captures have matching
548 source hashes, clean runtime diagnostics and closed owned API/Tracy ports
6142/8222 and 6144/8224. They are diagnostic captures, not FPS measurements.

The `fps-20261106/caster-inventory/` frozen-pose diagnostic paged every native
caster bound for the Avatar's four Sun cascades. An offline estimate projects
each local AABB with its captured transform and cascade matrix, retaining
uncullable or missing bounds. It assumes the High preset's 30–3000 m split and
pads the projected receiver rectangle by 0.05 NDC on each side. It identifies
36/1070, 16/1109, 20/1150 and 68/1219 candidate entries outside those padded
rectangles: 140/4548, or 3.08%, at this pose. Page fingerprints and matrices
match across the paused inventory; source hashes and runtime diagnostics pass.
This is a broad-phase estimate, not an exact native crop, draw-count or GPU-time
measurement, and does not cover moving views. It does not establish an FPS
gain sufficient to justify an extra per-frame visibility pass. The temporary
diagnostic fields are removed, the qualified normal executable is restored,
and owned API port 6146 is closed. The runtime log also reports a separate
`environment_sun_light_projection` run-condition warning for follow-up.

The celestial illuminance publisher now marks `SunState` changed only when its
semantic revision changes. Its focused resource-change test covers unchanged,
changed, invalid and withdrawn illuminance. The `fps-20261106/sun-change-normal/`
Griffin run retains identical full/cropped shadow pixels, valid physics clocks,
four authored checks, zero runtime diagnostic findings and all 548 source hashes.
Cropped FPS windows are 52.27 and 53.88; this separate run does not establish a
causal FPS improvement. The production directional-light GPU controls also pass.

Tracy captures `sun-change-tracy/` and `sun-change-tracy-baseline/` have the
same paused/live phase protocol; the older diagnostic executable has other
renderer differences. In settled paused windows both show zero projection
executions. In live windows the changed and older builds respectively measure
about 40.9 and 42.6 projection executions/s, averaging 0.0099 and 0.0090 ms.
This does not establish a Griffin performance improvement. The startup gate
warning remains, and the input responsible is not yet attributed. Both Tracy
captures retain matching source hashes and clear runtime diagnostics; all
owned ports are closed. The normal production executable is restored.

`fps-20261106/shadow-clear/` isolates the native shadow-pass floor with a
temporary application draw-wrapper probe. In one paused 2K Griffin process,
normal / suppressed / normal / suppressed / normal native shadow GPU means
are 6.3190 / 3.1145 / 6.2542 / 3.1163 / 6.7742 ms. Suppressed phases retain
the native shadow pass and attachments but issue no shadow draw commands;
their visible lander shadow disappears. Normal images before, between and
after are byte-identical. The remaining ~3.1 ms includes attachment clearing,
storage and other pass overhead; this experiment does not isolate the clear
operation alone. The probe is removed, source hashes and runtime diagnostics
pass, owned API/Tracy ports 6158/8230 are closed, and the normal executable is
restored. This diagnostic is not a product FPS result.

Native shadow attachments clear the complete map before receiver-bounded
draws. Measure the current clear and raster costs together on the production
scene; prior measurements of a different clear path do not establish current
performance. The 100 and 150 FPS targets remain unmet.

Initial normal cross-process crop windows show roughly 51.8→54.2–54.6 FPS but
worse tails. Typed registration removes an unnecessary exclusive-world access;
a subsequent cohort's baseline itself rises from 51.96 to 55.04 FPS, invalidating
its causal gain estimate. Raw `crop-normal-*` / `crop-final-*` receipts are
retained; only the same-process acceptance above establishes the reported gain.

The older long `live/` baseline drops from about 50 to 15–20 FPS alongside
sampled CPU clock collapse and 96–101 C temperatures. That is correlated
clock/thermal evidence, not proof of a firmware mechanism. No power or fan
policy is changed, and the short settled acceptance does not certify indefinite
thermal stability. The 150 FPS and 1 ms Modelica targets remain open.

An offline geometric estimate uses the previously captured fixed-pose camera,
Sun cascade matrices and caster inventory to compare the current projected
receiver rectangle with bands bounding its padded convex frustum projection.
With a deliberately conservative 0.05 NDC padding on both axes, two equal
horizontal or vertical bands retain approximately 92–94% of the rectangle's
area; four retain approximately 89–91%. This estimate is not native GPU timing
or a rigorous raster-proof implementation. Replaying every shadow draw for such
a small area reduction is not currently justified; no banded path is retained.
The inputs and per-cascade ratios are in `target/perf/fps-20261106/caster-inventory/run/estimated-bands.json`.
