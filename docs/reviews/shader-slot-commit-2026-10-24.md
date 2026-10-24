# Shader slot commit borrow

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
