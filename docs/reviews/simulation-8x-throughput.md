# Coupled simulation at 8x

Status: analysis; 8x Summer Space School acceptance remains open.

## Required result

The normal desktop host must complete 480 unchanged 60 Hz physical intervals
per real second at the 8x setting. The complete coupled critical path therefore
has a 2.083 ms average interval budget. Preserve equations, f64 values, solver
tolerances, event semantics, communication periods, native substeps, terrain and
render quality. A selected transport rate or a shorter native physics span is
not acceptance.

## Current evidence

The earlier ordinary desktop run reached 2.002x in Summer Space School and
about 3x at higher requests. The lightweight Modelica fixture reached 8x.
Neither result qualifies this coupled scene. The source cohort is
SummerSpaceSchool `ee2e5b9f7590fd32633fe9214d8f09998847cd24` with application
`3b41e00fd` plus the local native host changes.

The native Tracy 0.14.1 capture is retained in the ignored directory
`scripts/perf/captures/simulation-8x/baseline-tracy/`. Its saved file exports
successfully. The 35.9-40.0 second interval contains 1,021 complete fixed
cycles and 1,021 electrical worker transactions. Median inclusive costs are:

| Owner | Cost | Interpretation |
|---|---:|---|
| Complete `FixedMain` | 2.326 ms | Excludes asynchronous worker execution and out-of-tick admission |
| Native Avian `PhysicsSchedule` | 0.690 ms | Preserve physics quality; this is only one part of the cycle |
| Electrical Modelica worker | 2.325 ms | Integration, input admission and checked observation |
| Electrical integration | 2.061 ms | Primary worker optimization target |
| Electrical observations | 0.122 ms | Not the dominant solver cost |
| Electrical input admission | 0.113 ms | Preserve the atomic input transaction |
| Celestial endpoint | 0.258 ms | Mostly exact lunar ephemeris evaluation |
| Native pose projection | 0.095 ms per call | Two calls per fixed cycle |
| Physics geometry preparation | 0.109 ms | Separate from native integration |
| Fixed Rhai scenario driver | 0.107 ms | Attribute native traversal separately from authored evaluation |

These overlapping spans must not be summed as a measured coupled latency.
The electrical integration selection has 2,185 sampled CPU stacks at a verified
1 ms sampling period. Samples identify Rumoca's torn-tangent/JVP evaluation,
native program invocation, refresh projection and allocation. Function names
are incomplete, and 22% of leaf samples lack source attribution; they do not
establish one exact source-line saving.

The gate ended when its auxiliary focus window exited, before its ten-second
rate verdict. The trace is diagnostic evidence, not a passing rate cohort.
The interval also contains roughly one-second native `present_frames` waits.
Those are presentation waits, not CPU computation. They require a clean
focus/presentation comparison before attributing the remaining throughput cap
to hardware. Concurrent workloads are recorded in the capture receipt.

The uninstrumented development candidate activates the compiled scalar lane
path (`lanes=4`, `programs=88`, `tensor_lanes=1`, `native=true`). A thirty-second
8x request completes at 3.118915x over 30.934899 s. Its electrical worker mean
is 2.217 ms; recent fixed service median/p95 is 3.149/4.599 ms. Its native
physics-time conservation check passes, and registered runtime diagnostics
are clear. The window changes focus during measurement and presentation drops
to roughly 1 FPS, so it is not a stable-focus comparison.

A separate fifteen-second `--no-vsync` diagnostic restores approximately
60.61 FPS but completes only 2.359340x. Electrical worker mean is 2.555 ms;
recent fixed service median/p95 is 2.828/4.092 ms. Registered diagnostics are
clear. Concurrent builds and a foreign simulator remain active. These two
windows fail the rate gate and cannot establish an isolated batching speedup.
Raw diagnostics and screenshots remain under ignored
`scripts/perf/captures/simulation-8x/candidate/`.

The normal release meter build (`c238a763a546aba481a3873d155535e950c4108c7165f3630681328edea7c7e2`)
measured **7.1271x at 8x** over 30 seconds. Three of four authored checks,
including native physics-time conservation, passed; the requested-rate check
failed, while runtime diagnostics remained clear. Its final fixed-tick service
p50/p95 was 1.190/1.718 ms and electrical worker mean was 0.859 ms. It remains
below the required rate.

Two later 8x attempts with the revised host executor candidate measured 5.3137x
with ordinary background pacing and 5.0587x with `--no-throttle`; all clock
conservation checks passed, but each rate check failed. A fresh run of the
prior release candidate measured 5.5212x. These windows show that continuous
host updates alone do not close the gap. Their timing profiles and diagnostic
screenshots are retained under `candidate/admission/` and `candidate/release/`.
The current executor changes have not demonstrated a throughput improvement.

The final foreground attempt overlapped two builds in the separate
`../optimization` checkout and is excluded from performance comparison. No
foreign build or simulator was stopped. A clean ordinary desktop 8x verdict is
still open.

Rumoca's development-build torn sweep also performs an independent per-row
reference sweep and exact f64 comparison on every invocation. That check is
preserved. Development and ordinary release profiles must be qualified
separately before attributing their different execution costs to a solver
algorithm change. The application release profile is unchanged.

## Optimization order

1. **Rumoca integration:** inspect the existing torn-Jacobian/native JVP path.
   Reduce repeated small native invocations and reuse evaluator scratch at its
   authoritative owner. Preserve causal sweep order, complete value validation,
   event handling and exact replay. Do not replace the integrator or increase
   the communication interval. A useful engineering budget is at most 0.8 ms
   per electrical macro step; the measured 2.061 ms already occupies almost
   the entire 8x budget before other serial work.
2. **Application fixed work:** reduce the non-Avian portion of `FixedMain`.
   Compare the actual hot workspace callers at optimization level three;
   Cargo's dependency wildcard does not optimize workspace members. Inspect
   existing ephemeris and pose/geometry reuse before adding any cache. Exact
   epochs, source identities, physical input cadence and invalidation remain
   authoritative. Target complete fixed service below 1.2 ms, then verify
   actual overlap with the worker rather than adding hypothetical savings.
3. **Native presentation service:** qualify focused and background windows
   independently. The current host services pending cycles during the render
   handoff but still synchronizes application clock sampling with render return.
   Attribute this wait separately. Any further decoupling must retain one
   authoritative World, one clock and one receipt consumer, and preserve native
   extraction events, scene retirement and physical easing. Skipping physics
   or discarding demand is not an optimization.

## Gates

- Use the ordinary production binary for sustained 8x acceptance; retain a
  separate instrumented run for attribution. Record exact source hashes,
  settings, native diagnostics, terrain readiness, focus and competing loads.
- Require the authored transport verdict plus an independent monotonic-clock
  measurement. Longer `--sample-seconds` windows are supported by the shared
  driver. Keep passing lower rates separate from a failed 8x request.
- For solver changes, retain differential f64 state/output checks, the existing
  exact replay references, input rejection, initialization and event cases.
- For host changes, qualify pause/resume, exact manual stepping, scene
  replacement, presentation time, recording and clean render diagnostics.
- The final result requires a rendered Summer Space School run at 8x, with the
  same authored source and physics/solver/render quality.

## Candidate under verification

Local Rumoca optimization was explicitly authorized. The isolated
`../rumoca-simulation-8x` checkout starts at the pinned `7191bffc` revision.
It composes the existing checked tangent-lane programs with the existing
native Jacobian compiler. Supported lane programs execute together; native
width limitations retain the existing one-direction form during preparation.
Admitted native execution errors propagate instead of becoming reference
execution. Focused tests compare exact f64 bits, reduced native invocation
counts, original source provenance and width-decline behavior. The fixed-runtime
owner tests pass 105 checks. These are mechanism checks, not an 8x verdict.

The application candidate also raises the traced workspace-owned fixed-cycle
callers to the existing level-three development optimization policy. Debug
assertions, overflow checks and all numerical/quality settings remain enabled.
The local Cargo overlay points at the isolated Rumoca source; it is not a
published dependency revision. Prepared-model disk caching can be unavailable
under that development overlay, so its loading time cannot qualify a portable
Git-pin release.

The transport gate now requires both authored and independent wall-clock rates
within one percent, with configurable sustained windows. A 6.8x result cannot
qualify the requested 8x setting.

Each rate runs for a fixed three-second warmup before sampling. It retains
pending fixed time and changes no simulation state beyond the requested rate;
this excludes the rate-change presentation transient. The performance HUD is
enabled in the measurement session. An optional Modelica HUD gate checks ten
running, paused and cleared-scene assertions after the throughput window.

The Modelica execution owner publishes total worker service per physics step,
per-instance mean service time, and completion throughput over a native one-second
wall-clock window. Its window and typed `ModelicaHealthSnapshot` are registered
scene resources. The shared engine-health exposure is their presentation
boundary, consumed by the existing performance HUD. Worker counters and session
identities remain owned by the existing admitted-receipt diagnostics; this
presentation adds no participant receipt consumer or simulation clock.
The continuous measurement math has no authored routing decision requiring a
Rhai hook. Native qualification passes the ten running/paused/cleared HUD checks
and the mixed-rate network's 44 clock checks. The captured single-engine and
60/30 Hz two-model views show measured milliseconds per physics step. This
qualifies the diagnostic display, independently of sustained 8x throughput.

The status-bar meter displays measured Modelica worker milliseconds per physics
step. The execution owner sums validated worker service during its wall-clock
window and divides by completed physics ticks; the tooltip includes the largest
per-instance mean service cost. Completion throughput remains available through
the diagnostic exposure, independently of the visible cost readout.
