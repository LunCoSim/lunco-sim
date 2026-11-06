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
