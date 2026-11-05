# Fixed-output execution and current production measurements

Rumoca's fixed root and visible-output readers share one checked executable
selection owner. The consumer pins published Rumoca
`e325c2c12d2115e78f481f40701784b510dcca3c` and the four maintained Bevy
renderer/light crates at published `eabfa346c153337a287c0de0b3513efe6b820e04`.
The application integration is local; no LunCoSim push is part of this work.

## Architecture and verification

Private `PreparedOutputSelection` owns complete selected programs, checked
output placement and optional compiled execution. Prepared root and visible
plans carry this capability with their semantic metadata. Their readers use the
same evaluation path. Separate root preparation and visible per-row native
caches are removed. Dynamic guarded event/discrete membership uses the common
selected-program admission helper while retaining its own membership semantics.
Direct observations, expression deduplication, grouped outputs, source order and
fresh state/parameter/time/table inputs remain intact. Compilation decline uses
the prepared interpreter; admitted native errors propagate. No numerical result
or COM term is cached. This is a generic numerical mechanism, without a new
Rhai policy hook. See the [runtime contract](../architecture/29-rumoca-runtime-contracts.md)
and [SDK review](../../../rumoca/docs/reviews/fixed-output-selections-2026-11-05.md).

Observable behavior is tested through existing authored Rhai scenarios in the
production binary. Python only orchestrates their owned sessions and API calls.
The SDK's asset-free Rust tests cover mechanisms Rhai cannot directly observe:
native call counts, prepared ownership, bounds and exact floating-point bits.

- 567 solver tests pass; two manual diagnostics remain ignored. Solver/native
  facade Clippy passes for library and test targets.
- All production stages pass: 264 authored checks plus two expected-fault scenes,
  covering commands, inputs, parameters, observations, lifecycle, initialization,
  clock/receipts, compiled metadata and plume consumers.
- The fixed 20-model canary keeps all classifications: twelve completed
  strict-high comparisons, six ToDAE refusals and two structural solve refusals;
  no missing/skipped comparisons. All 562,411 values and 16,841 timestamps match
  the fresh baseline bit for bit. This is Tier 1, not full 566-model parity.
- Five alternating native executable pairs reduce the 384-output/512-fresh-time
  visible fixture median from 15.572587 to 0.832622 ms (94.65%). Every output is
  checked bit for bit. Root control ranges overlap (0.832–1.395 and
  0.839–2.133 ms), with medians 0.945062 and 1.046080 ms; no root gain is claimed.

## Normal production measurements

The canonical scene is explicitly
`twins/astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda` in
`/home/rod/Documents/models/lunar-base-model`. All three windows qualify at
High/2560×1440 with no vsync/throttle, thirteen participants, terrain/readiness
and physics handoff satisfied, and zero runtime errors. All 546 guarded source
files are identical. No models, precision, timestep, substeps or quality settings
were changed.

| Window | Frames | FPS | Frame p50 / p95, ms | MainPropulsion warm ms/job | Aggregate worker ms/tick |
|---|---:|---:|---:|---:|---:|
| Fresh baseline | 454 | 44.993 | 21.683 / 29.099 | 1.471 | 3.139 |
| Final candidate | 455 | 44.968 | 21.804 / 28.156 | 1.545 | 3.362 |
| Final candidate repeat | 523 | 50.315 | 19.481 / 24.001 | 1.409 | 3.053 |

Warm per-participant service uses cumulative counter deltas after readiness;
aggregate values are the median reported worker work per authoritative physics
tick. These are attribution, not coupled-step latency. Fixed-tick service p50
is 4.188, 4.173 and 3.574 ms respectively. The candidate changes both Bevy
light/PBR admission and SDK batching, so these windows do not isolate either
change. Host workload variation remains a timing limit. The repeat is better,
but the first candidate is flat: no stable scene-wide FPS or Modelica gain is
established. Neither the 1 ms Modelica target nor the 150 FPS target is met.

The baseline app is `df7660049`, SDK `87b117a`, with released light/PBR crates.
The final candidate uses published pins above and was built from app
`31e1a0cdf` plus the intended Rumoca pin/document changes. Its normal binary
SHA-256 is `802dd38a63133ca532304dda35f210c063530b3f2d7e0cdead329d658b190627`;
the baseline is `aba9fa5fb3e9e3f1ce9b518bd18cb6a5b106d2fced18d65804b07647dd334491`.
The candidate remains installed at `target/release/luncosim`.

## Tracy findings and next work

The diagnostic capture uses app `31e1a0cdf`, patched Bevy `eabfa34` and SDK
`87b117a`, before the shared visible-output change. It is not a final-candidate
capture. The matching Tracy 0.14.1 collector starts before the app; sampling is
1 kHz. The qualified ten-second region ends 100 ms before the final screenshot.
Profiler FPS is excluded from acceptance.

1. **Modelica integration is the main remaining worker target.** MainPropulsion
   averages 1.496 ms/job: integration 1.176 ms, observations 0.306 ms and input
   admission 0.0064 ms. Native integration samples across all workers include
   allocation, checked refresh values, tangent evaluation, JIT Jacobian and
   `powf`. The sample stack does not reliably recover callers through generated
   code: 186 of 188 allocator samples lack a recovered domain caller. Attribute
   allocation at the solver owner before changing scratch storage or Jacobian
   execution. Refresh checks traverse refreshed targets, not every state slot;
   preserve finite-value admission and exact dependency/coordinate validity.
   MainPropulsion needs roughly another 0.4–0.55 ms reduction to reach 1 ms in
   these normal windows; aggregate 1 ms requires broader work across participants.
2. **Telemetry still hashes names in a hot path.** Runtime retention averages
   1.646 ms/frame. Its 552 scoped samples include scalar-history lookup (160),
   SipHasher writes (73), signal-reference hashing (11) and string hashing (10).
   Current due-sample recording visits named channels; pending admission also
   retries rows when the recorder capacity is exhausted. Investigate recorder-
   owned retained bindings and capacity/catalog-revision-driven admission through
   the existing signal owner. Preserve sampling rate, archives, reset leases and
   the no-new-tick wake after scene reset. Strings remain appropriate at authored
   discovery/API boundaries; repeated numerical recording should consume checked
   owner bindings. This optimization is not implemented here.
3. **Keep exposure policy in Rhai; reduce repeated fact conversion.** Visibility
   and properties average 1.231 and 0.993 ms/call respectively (400 calls each).
   Scoped samples contain substantial allocator/BTreeMap construction. The surface
   preparation calls two typed hooks over rich facts. Prepare immutable scripting
   facts once per valid snapshot/revision, retaining the exact scene lease and
   execution context; let Rhai continue making policy decisions. These scopes
   overlap parent work and must not be added blindly to frame time. This is a
   measured next investigation, not a completed optimization.
4. **GPU opaque and directional shadows dominate the render target.** Mean opaque
   time is 9.606 ms; camera early directional shadows are 7.642 ms. The nested
   `per_view_shadow_pass` is the same work and must not be counted again. The
   settled region has no recorded local/shared-shadow events. Prepass is 0.546 ms,
   bloom 0.262 ms and clustering 0.072 ms. Investigate opaque shader/lighting
   cost and conservative caster/instance admission while preserving the image.
   Directional shadows alone exceed the 6.67 ms budget for 150 FPS. GPU zone
   sums are not a complete frame measurement, and render receipt waits are not
   pure CPU computation. No shadow result reuse is admitted without valid
   scene/light/camera/deformation invalidation.

## Reproducible evidence and session state

Evidence is retained under `.cache/perf/latest-20261105/`: independent normal and
native executable pairs, build receipts, `normal-comparison.json`,
`native-fixed-selection-pairs.json`, `canary-delta.json`,
`solver-tests-final-receipt.json`, `native-clippy.log`, `production-gates.json`
and stage logs. Tracy contains `landing.tracy` (303,748,457 bytes), exact ELF,
process/JIT mappings, CPU/GPU exports, `roi.json`, `zone-summary.json`,
`gpu-summary.json` and scoped sample-resolution evidence. The before/after
canaries are retained under the Rumoca checkout's `target/msl/`.

Normal sessions use ports 5850, 5852 and 5853; diagnostic uses API 5851 and Tracy
8170; authored gates use 5906–5917. Every owned app exits through its API and all
these ports are released. Other users' sessions are untouched. Bevy branch
`fix/gpu-shader-buffer-stride` and Rumoca `main` publication SHAs are verified
against their remotes. LunCoSim is integrated locally only.
