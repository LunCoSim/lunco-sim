# Shader slot commit borrow

## Authored type and live samples

Scalar admission uses the authored `F32` contract. The exclusive slot commit
reads the previous live sample only for no-op change detection, then writes the
admitted value through `set_live_slot`. The previous override's variant does
not determine permission. This retains the registry's live ownership, metadata,
range and atomic-preflight checks without a second, inconsistent type assertion.
The regression reproduces a panic on the baseline by replacing an `I32` live
override on an authored float; the admitted replacement succeeds after repair.

The final focused command
`cargo test --locked -p lunco-usd-sim-shader --lib ports::tests -j 4 -- --nocapture`
passes eight functional checks, with one manual diagnostic ignored. The regular
production build passes in 1m41s, SHA-256
`7b736a03bc7951d2cd0559aa56dd811a33b0c8ce64b589473da9574acc6f0386`.
The production plume scene passes all six authored checks on PID 1950746/API
5230 and the windowed no-VSync host repeats all six on PID 1955005/API 5233,
with clear registered diagnostics. Its inspected screenshot shows the success
notification and fixture ground; that view does not establish plume appearance.
Both processes exit normally and release their ports.

The first windowed attempt, PID 1953464/API 5232, leaves VSync enabled and times
out before the authored verdict, despite clear readiness. It closes through API
Exit. A retained timing observation contains only 63 completed ticks and 42.832
seconds of pending simulation time. The no-VSync rerun records timing and frame
samples and completes the same authored contract. This isolates a presentation
configuration difference, not the cause of the earlier pacing delay.

Normal Griffin PID 1951905/API 5231 completes touchdown and four-pad handoff,
five seconds of full terrain readiness and a measured window with at least 240
fresh ticks. Sources stay unchanged and registered diagnostics are clear. The
inspected image retains the landed vehicle, terrain and shadow; the app closes.
First physics readiness is 29.612 s, landed eligibility 100.046 s, coupled
median/p95 3.898/4.641 ms and frames 12.920/16.016 ms. A separate Tesla session
is active during startup; these are contention-qualified observations, not a
speedup or whole-mission acceptance claim. All performance targets remain open.

A declaration-query experiment is removed after five alternating pairs measure
less than one percent improvement. It does not justify a new helper or retained
runtime path. The temporary baseline test hard link is removed. Raw observations
remain ignored under `.cache/codex-evidence/performance-20261023/`, in
`shader-declaration-comparison/`, `shader-live-contract-*` and
`griffin-shader-live-contract-normal/`. No additional Tracy run is required for
this correctness repair; existing qualified captures retain the attribution.

## Component borrow

The material port backend retains one native `Mut<ShaderLook>` from slot
inspection through commit. Its shared dereference reads the current value
without marking the component changed. A different scalar value reaches the
existing `set_live_slot` owner through the mutable dereference. Live admission,
owner precedence, atomic transaction ordering, f64 validation and the explicit
GPU f32 boundary remain unchanged. No API, cache or lifecycle policy is added.

`cargo test --locked -p lunco-usd-sim-shader --lib ports::tests -j 4 -- --include-ignored --nocapture`
passes eight checks before and after the change. Existing checks cover unchanged
values, stable topology, missing ownership, untyped fields, integer reads and
actual registry transactions. The diagnostic measures 393,216 writes per phase:

| Transaction phase | Baseline | One component borrow |
|---|---:|---:|
| Constant scalar | 86.121 ms | 80.538 ms |
| Changing scalar | 100.417 ms | 79.647 ms |

These are bounded diagnostic observations, not a whole-scene improvement claim.
The regular production build passes in 1m01s, SHA-256
`74772d005491345f1eff540e6655ea1f51c69c4d80284b3d7356a85ec1605734`.
The production `lander_plume_activity.usda` scene passes all six authored checks
on owned PID 118437/API 5184 at tick 242, exits zero and closes. It exercises
connected delivery, explicit writes, oversized-value rejection and next-tick
restoration by the connected source.

Owned Griffin PID 119921/API 5186 completes landing, five seconds of full terrain
readiness and a ten-second window containing at least 240 fresh fixed ticks.
Sources remain unchanged, runtime diagnostics are clear and the final terrain,
lander and shadow image is inspected. The app closes normally. Whole-tick
median/p95 is 3.966/5.312 ms; frame median/p95 is 11.669/16.566 ms. Native Avian's
last retained step window is approximately 0.35 ms, so it does not explain the
complete coupled-tick cost. Another worktree's body-view session and browser
work remain active; these are contention-affected observations. The first ready
observation is 20.540 seconds; the 43.036-second landed milestone includes descent.
The earlier API 5185 attempt encountered the known source-retirement interval
before a stage existed, closed normally and is excluded. The measurement gate
now treats that exact pre-admission observation as pending; it still rejects
loss of readiness during an admitted measurement.

Raw evidence stays ignored under
`.cache/codex-evidence/performance-20261023/shader-write-borrow-*` and
`griffin-shader-write-borrow-normal*`. The unused stage-import warning remains.
Complete loading, coupled-tick and graphics timing requirements remain open.

## Landed native attribution

The separate Tracy build passes in 1m13s. The matching adjacent 0.14.1 collector
starts before owned PID 135029/API 5187 on profiler port 8425 at 1 kHz. Landing,
full terrain readiness, five seconds of soak and at least 240 fresh measured
fixed ticks complete. The inspected image and diagnostics pass, sources remain
unchanged, and app and collector close normally. The admitted source hashes,
model commit and native counts match the preceding shader-input capture:
638 bodies, 20 dynamic, 21 sleeping, 777 colliders, 31 joints, 315 graph pairs
and zero orphan pairs. Rendering uses the native GPU preparation path.

Native framing and export verify 18,849 blocks, 400,413,842 compressed payload
bytes and 400,489,244 file bytes, the exact owned PID and 1,000,000 ns sampling
period. The last 240 complete FixedMain instances span 58,695,170,305 through
63,150,772,739 ns. Instrumented whole-tick median is 5.484 ms. Inclusive write
median is 0.880 ms and complete propagation is 1.018 ms; the preceding capture
observed 1.034 and 1.239 ms. These are different render/host cohorts, so this is
bounded attribution rather than an isolated shader speedup claim.

The same window shows response handling at 0.877 ms per invocation (478 rows),
native physics at 1.454 ms and celestial endpoint publication at 0.498 ms.
Completion admission includes response handling; do not add parent and child
costs. Native GPU preprocess bind-group preparation is 2.160 ms per frame,
and shadow instance-buffer preparation is 1.388 ms. These are further graphics
leads, not evidence that a quality reduction is needed.

The largest constructor is 3.500 seconds, with 16,414 native cache hits in
16,482 attempts and 68 compilations. Its 3,122 selected CPU samples contain one
enclosing-function leaf in `program_index_for_output`, versus 157 of 3,133 in
the preceding capture. Shared-value construction, IR copies, Cranelift and
native key hashing remain visible; complete loading is still far above target.

The write selection has 220 CPU samples across 240 zones. Saved exact symbol
indexes and PIE mappings resolve 163 executable leaves and 57 external leaves;
live admission, name indexes and repeated ECS lookups remain prominent. The
response selection has 440 samples across 478 invocations: 42 executable leaves
in SipHash, sixteen in standard-map membership and ten in its random-state
hasher. The input and parameter lookup costs motivate the subsequent
Modelica live-map change, which retains complete live classification and audits
all consumers. The worker sorts input names before dispatch.

Per-process pressure telemetry records peak resident sizes of 5,218,732 KiB for
the app and 1,753,148 KiB for the collector, with about 0.223 seconds of global
full memory pressure over the complete run. This does not diagnose the preceding
unqualified capture's long stall. Raw native data and summaries remain ignored
in `griffin-shader-slot-landed` and `scripts/perf/captures/`.
The normal production binary is restored after capture by the standard build
in 2m21s, SHA-256
`bdee65e9bedba599d4d823ea86cc12ffaed69373625dd6632a3ad9741eb7159d`.
