# Griffin loading profile — 2026-11-26

## Desktop profile of the integrated release

Local merge `5235460ca`, built from `usd`, reached lander physics readiness in
**18.40 seconds on an empty Modelica cache and 11.99 seconds on the next open
in a fresh process**. The first ready screenshots completed at 18.71 and
12.27 seconds. This was the same surface-operations Twin and private desktop
setup as the paired comparison below. The binary SHA-256 was
`b132e0f846156423165f6e06250f3de1f0b7f1eeb6f1685e8a0340c`.

| Merged desktop run | Shared initialization | Prepare or read shared component | Longest live constructor | DAE / component hits |
|---|---:|---:|---:|---:|
| Empty Modelica cache | 5.30 s | 3.55 s | 3.02 s | 0 / 0 |
| Warm Modelica cache | 3.61 s | 1.81 s | 2.91 s | 16 / 16 |

The warm run reused 12,444 native functions and compiled 29,576. All 199
time-zero values matched the paired builds. Both runs reached physics readiness
with 15 active participants, zero scene diagnostics and matching source snapshots.
A separate desktop run observed `landing_handoff = 1.0`. It reached lander
physics readiness at 17.13 seconds and passed the full handoff and terrain
qualification at 64.48 seconds. The integrated executable also passed the 47
authored initialization and replacement checks and the expected-fault fixture.
The desktop displayed a separate download warning for the Earth and Moon color
maps during these physics measurements.

The merged build also includes intervening runtime and rendering changes.
Use the paired runs below to estimate the effect of the Modelica changes alone.

## Production result with checked component and native function reuse

The release from the `usd` checkout reached Griffin lander physics readiness
in **17.48 and 14.04 seconds** on two warm desktop opens. The immediate
control reached it in **17.35 and 17.02 seconds**. The median fell from
17.19 to 15.76 seconds (8%). Both candidate opens beat the 20-second target.
The paired comparison does not establish a twofold end-to-end improvement.

| Fresh process and Modelica cache | Control ready | Candidate ready | Control shared initialization | Candidate shared initialization |
|---|---:|---:|---:|---:|
| Empty, headless | 24.13 s | 21.76 s | 7.41 s | 7.75 s |
| Warm, private desktop, candidate first | 17.35 s | 17.48 s | 7.63 s | 4.94 s |
| Warm, private desktop, control first | 17.02 s | 14.04 s | 7.78 s | 4.56 s |
| Warm, headless | 15.84 s | 11.75 s | 8.73 s | 3.75 s |

The control was release `8906e198` with Rumoca `dc3574345`. The candidate
pinned `429e3ff2765b` of the LunCoSim Rumoca fork. Each run used
`scripts/perf/twin_startup.py --startup-only` to open
`twins/astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda` at model
revision `269273031bec`. Timing starts at `OpenFile` from a ready bootstrap
and ends at the first authoritative physics state for
`/Griffin1SurfaceOps/Lander`. The desktop runs used private X displays and
the NVIDIA Vulkan renderer. Each binary had its own Modelica cache and
shared the other asset directories. Other simulator and linker processes
were active on the machine during the first candidate desktop run.

Every paired run admitted 15 participants, certified the eight coupled
members, reported zero scene diagnostics, preserved the source snapshot,
and released its process and API port. Across all four candidate processes
the 115 named probes and 84 state starts at time zero matched bit for bit.
The SHA-256 of the sorted role/name/binary64 snapshot was
`2550c62f1d33e34085ce5f25c155fc9a923d038b38b0d0b2e4c43d85844614fe`.
In a separate run a valid cached record placed under a different key
was rejected and rebuilt. The lander reached readiness with all 15 participants.
The initialization network and Twin-replacement gates passed 47 authored
checks. The inconsistent system produced its expected terminal
`modelica-initialization-failed` fault.

**Remaining loading time.** Both warm desktop runs reused all 16 strict DAEs
and respectively 15 and 16 checked components. Reading and validating the
shared component took 2.45 and 2.30 seconds. Another 2.49 and 2.26 seconds
went to the remaining initialization and observation. The longest live
constructor took 5.91 and 3.29 seconds, overlapping the other constructors.
In the second desktop run Cranelift reused 12,460 of 42,020 native function
compilations across participants. The popup first reported source work, then
eight participants in the shared solve and finally progress starting eight
sessions. The captured popup showed elapsed scene time in both later phases.

To reduce a repeat desktop open further, measure the roughly 29,560 fresh
native functions by function family and the independent work that can start
before the shared solve returns. The 2.3–2.5 seconds spent reading and
validating the shared component also warrant a split between wire decoding and
proof reconstruction. Preserve the fresh per-instance solve and compare the
same named time-zero values on each path. On an empty headless cache, shared
initialization took 7.75 seconds, including 4.98 seconds to prepare the
component. Disk publication had no measurable queue wait for that component.

## Earlier headless comparison

The updated release reached first physics readiness in **15.01 and 16.33 s**
on two warm headless loads, compared with **46.07 and 34.34 s** in alternating
control runs. With an empty Modelica cache, the control took **33.67 s** and the
updated release took **16.80 s**. All measurements opened the exact surface
operations scene.

## Profile after the popup merge

The production binary reported `8906e198` with the simulator-compatible Rumoca
pin `dc3574345`. The exact Griffin surface-operations scene and model revision
were the same as in the preceding comparison. Each production measurement
started a fresh application, opened the scene from a ready bootstrap, and ended
at the first physics-ready observation of the lander.

The windowed regular release reached physics readiness in **34.89 s** with
all 16 compiled artifacts already on disk. At 12.0 s the popup reported four
sources still loading. At 12.3 s it switched to solving initial conditions for
eight participants. That shared worker took 14.36 s. At 27.2 s the popup
showed the eight live sessions starting. The longest took 7.06 s. These counts
describe three distinct stages. The window used a private X display and the
NVIDIA Vulkan renderer.

The table compares fresh headless processes. The diagnostic skipped only
zero-length byte comparisons in the simulator process. Each warm run reused
all 16 compiled artifacts. Each cold run compiled all 16 with its own empty
Modelica cache and shared terrain and library assets.

| Cache and pair order | Regular ready | Diagnostic ready | Regular shared solve | Diagnostic shared solve |
|---|---:|---:|---:|---:|
| Warm, regular first | 18.67 s | 13.98 s | 7.16 s | 5.61 s |
| Warm, diagnostic first | 14.91 s | 12.72 s | 6.98 s | 5.32 s |
| Warm, regular first | 13.98 s | 15.53 s | 7.03 s | 5.95 s |
| Cold, diagnostic first | 40.87 s | 25.94 s | 13.90 s | 7.61 s |
| Cold, regular first | 43.49 s | 47.67 s | 14.44 s | 13.38 s |

Across the three warm pairs, median readiness fell from 14.91 to 13.98 s
and the median shared solve from 7.03 to 5.61 s. The shared solve improved
in every warm pair. In the third pair the longest live session took 5.62 s
with the diagnostic and 2.97 s with the regular executable, reversing the
overall result. Competing Rust builds varied during the cold runs, so the
pairs cannot establish the cold-start saving. All runs reached lander physics
readiness with 15 active participants and zero diagnostics.

The 1 kHz Tracy run reached readiness in 24.21 s. Its worker receipt recorded
10.43 s for the eight-member coupled initializer. Eight members then started
live sessions. The three longest constructors overlapped and took 7.20, 6.46
and 6.07 s. Across all 15 live constructors, 4,553 of 10,491 sampled leaves
were in Cranelift or register allocation (43.40%). Formatting accounted for
4 of 9,234 coupled samples and 30 of 10,491 live samples. All 16 DAE lookups
hit the disk cache and took 301 ms in aggregate. A frequently executed wiring
membership zone took only 4.7 ms over 19,173 visits.

### Reuse of the checked component

LunCoSim caches the compiled DAE. On the next launch Rumoca lowers that DAE
again, builds the checked FMI component and creates a new numerical instance.
The experiment used the eight-member DAE from the exact scene's cache and its
declared defaults. Both routes created a fresh interpreted RK45 host and read
all initial observations. Scene-specific tunable overrides were outside this
isolated experiment.

| Isolated coupled initializer, two fresh processes per route | Lower or replay component | Initialize and observe the fresh instance | Total wall time | Process CPU time |
|---|---:|---:|---:|---:|
| Build from DAE | 15.03 s | 8.44 s | 23.47 s | 23.68 s |
| Read and validate prepared component | 6.18 s | 8.55 s | 14.73 s | 14.94 s |

The runs were interleaved during a release build. The checked-replay timing
includes reading and decompressing the artifact and checking its contents
through Rumoca's existing reader. The prepared JSON was 33,461,018 bytes and
1,145,041 bytes with Zstandard. Re-encoding the reconstructed component produced
identical bytes. All 7,855 finite initial observations, including 115 generated
probes, matched bit for bit across both routes. Initial artifact creation is a
miss cost. One observed encode, compress and write took 0.69 s.

For repeat loads, extend the worker's bounded artifact cache with Rumoca's
checked component representation. Include the source and library identity,
compiler and wire versions, solver profile and exact input, parameter and start
overrides that affect the component. Expose preparation and replay from
Rumoca's native simulation facade so both routes initialize a fresh instance
through the existing solver owner. Benchmark the integrated hit and miss paths
with changed inputs and consecutive Twin loads.

### Repeated scalar type checks

Matching Ubuntu libc debug symbols identified `memcmp` in 2,083 of the
9,234 coupled-zone samples (22.56%). A separate diagnostic sampled the
verified short-comparison assembly path in the isolated initializer. In a
second sample, 1,624 of 1,647 comparisons had zero length. Rust's slice
equality called `bcmp` for empty dimension arrays. Most came from typed
register writes and binary operations. The isolated caller and length samples
have a different denominator from the production Tracy capture.

A process-local diagnostic returned equality immediately for zero-length
`bcmp` calls and forwarded nonempty comparisons to the installed libc. In
the initializer-only experiment, this reduced median CPU time from 23.80 to
20.90 s across two interleaved pairs. All 7,855 initial values still matched.
The scene comparison above exercised the same diagnostic in the owned simulator
process. This estimates the opportunity across callers on this Linux host.
The shipped executable uses its ordinary comparison implementation.

For a portable fix, specialize scalar equality in Rumoca's `SolveValueType`
and the DAE dimension checks implicated by the caller samples. A scalar and a
one-element tensor must remain distinct. Preserve exact rank and extents for
tensors and the scalar numeric type. Evaluate the localized implementation on
the scene before assigning it the diagnostic's observed time saving.

The local evidence is in `target/perf/griffin-merged-profile/`. The `tracy/`
directory contains the owned capture, process maps, per-zone samples and
construction receipt. `prepared_load.rs`, `prepared-cpu-pairs.jsonl`,
`comparison-callers.txt` and `length-addresses.txt` record the component and
scalar-check experiments.

## Follow-up experiment

The control executable identified itself as `a2adc563-dirty`, with Rumoca
`2a3716da5`. The candidate was built from `usd` at `354606a5-dirty` with
Rumoca `dc3574345`. Both used regular release code generation, the same Twin
revision, and the production startup driver. The candidate also contained
concurrent presentation edits. The timings measure the combined release change.

| Cache and execution order | Control ready | Candidate ready | Control coupled solve | Candidate coupled solve |
|---|---:|---:|---:|---:|
| Warm, control then candidate | 46.07 s | 15.01 s | 30.24 s | 7.35 s |
| Warm, control then candidate | 34.34 s | 16.33 s | 20.71 s | 7.41 s |
| Empty Modelica cache per executable | 33.67 s | 16.80 s | 17.70 s | 6.46 s |

The warm median improved from 40.20 to 15.67 s (2.57×). The cold pair used
separate empty Modelica cache directories and shared the existing library,
terrain and texture caches. The first candidate run before the alternating
comparison took 28.17 s and compiled all 16 classes. CPU contention varied
during the session. Earlier contended and instrumented runs below are reported
separately.

Rumoca now keys shared-value proofs by the binary operation structure and the
issuing pure-call owner. The one-shot coupled session uses the interpreter for
its initial observation. Live participants prepare their configured execution
backend.

### Formatting cost after the fix

Two 1 kHz Tracy captures measured CPU samples inside the same worker zones.
The table counts the innermost sampled function across all symbols. Unresolved
addresses stay in the denominator. Each capture covered one coupled observation
and 15 live session constructions.

| Worker zone | Formatting before | Formatting after |
|---|---:|---:|
| Shared initial observation | 13,591 / 55,645 (24.42%) | 4 / 16,577 (0.02%) |
| Live session construction | 16,017 / 49,967 (32.06%) | 23 / 16,921 (0.14%) |

The formatting count includes Rust formatting functions and numeric-to-text
conversion. The captures measure CPU distribution. The alternating release
runs above measure user-visible latency. In the updated trace, Cranelift and
register allocation accounted for 7,914 of 16,921 live-construction samples
(46.77%) and zero coupled-observation samples. Repeated native preparation
in the persistent live sessions is the next candidate for measurement.

The old popup counted unresolved source requests until the coupled result
arrived, then switched to unfinished live participants. In the private-window
control it showed four sources from 28.43 s and eight participants from
104.89 s. The new popup uses the coordinator's fixed member set during this
transition. The private-window candidate reported the eight-member coupled
solve at 4.05 s, live session construction at 11.65 s, and physics readiness
at 15.53 s. This windowed diagnostic included progress queries and screenshots.

Every follow-up timing run reached 15 running and six idle co-simulation
participants, with zero diagnostic errors or warnings. The positive production
initialization and scene-replacement scenarios passed all 47 authored checks.
The inconsistent network reported the expected terminal
`modelica-initialization-failed` fault.

The repeated runs are recorded under `target/perf/griffin-{control,candidate}-*`,
with source hashes, worker receipts and process retirement in each `session.json`.
CPU captures and zone-filtered sample summaries are under
`target/perf/griffin-repeat-tracy/` and `target/perf/griffin-issuer-tracy/`.
The rendered popup is recorded in `target/perf/griffin-popup-settled/initial-equations.png`
and `target/perf/griffin-popup-settled/starting-sessions.png`.

## Workload and controls

Production `luncosim`, release, application `a2adc5636df87d03eb569fcdca3810bd9952127c`, Rumoca fork pin `2a3716da596e18598e21a3683b1ff6d13ee650a0`, model repository `269273031bec2906b7e6e2ecf15f6d818f45f750`. The retained binaries were built in the adjacent optimization checkout at the same application revision and copied into this checkout for the measurements. The uninstrumented SHA-256 is `3fc5fd5fb15491e3bc8b35064c5fab011d3746ca5209fcf55f6bc4a037017d3a`; Tracy SHA-256 is `e742479de81c107cb01c32e512779a1f49d0783b7562617b95999912ddf3265a`.

`scripts/perf/twin_startup.py --startup-only --headless --scene scenes/griffin_1_surface_ops.usda` used `OpenFile` on the owning `astrobotic-griffin-1` Twin, after a verified ready bootstrap scene. It observed `/Griffin1SurfaceOps/Lander` and the readiness gate atomically in one World snapshot. The application bootstrap took 0.643 s; the timed interval starts at the authorized scene-open command and ends at first physics readiness. The four runs each observed 15 running and six idle co-simulation participants, zero runtime diagnostic errors/warnings, an unchanged 602-file source snapshot, and the owned process and API port retired on exit. Existing asset and library caches were preserved. The first run compiled the Modelica artifacts; the trace runs and regular repeat subsequently hit the compiled-artifact disk cache. These are not a controlled cold-versus-warm timing comparison.

| Run | First physics-ready | Coupled observation | Slowest live solver session |
|---|---:|---:|---:|
| Regular release, API 5497 | 38.114 s | 19.568 s | 8.383 s |
| Tracy, API 5499, profiler 5501 | 89.431 s | 49.734 s | 29.317 s |
| Tracy with native maps, API 5500, profiler 5502 | 77.522 s | 41.044 s | 24.765 s |
| Regular warm release, API 5503 (contended) | 88.935 s | 46.075 s | 31.612 s |

The Tracy runs overlapped compilation workloads and incurred profiler overhead; use them only for attribution, not performance acceptance. The regular warm repeat used the exact original release binary, but the host had unrelated active Rust compilation and browser workloads; its receipt confirms the same critical path despite cache hits. The coupled observation and slowest member construction alone total 77.687 s (87.4% of that run). Do not interpret the warm time as a cache regression. The follow-up private-window run used Xvfb. The table above contains headless runs only.

## Critical path in the regular release run

Wall-clock offsets below use the `OpenFile` log timestamp (08:57:09.697 UTC) and owner lifecycle logs; the readiness harness reports the precise overall duration. Work that overlaps is included only once in the critical wall-time column.

| Time from open | Phase | Wall time |
|---|---|---:|
| 0.00–3.01 s | Twin mount, USD document/projection, source admission, epoch, terrain collision support | 3.01 s |
| 3.01–7.43 s | Remaining participant compilation/interface admission and early independent live constructions | 4.41 s |
| 7.43–9.92 s | Generated `LunCoCoupledInitialization` compile and compiler-actor completion | 2.50 s |
| 9.92–29.58 s | Coupled initializer construction and requested observation for eight members | 19.57 s worker service; ~19.66 s wall |
| 29.58–37.98 s | Parallel member session construction and certificate | 8.40 s wall |
| 37.98–38.11 s | First physics admission and observed readiness | 0.13 s |

USD/terrain support is not the dominant blocker. The 19.568 s observation plus the 8.40 s post-observation interval account for 73.4% of the measured scene-open duration. The coupled initialization worker queue wait was 0.029 ms. The live construction receipts include:

| Participant | Session construction | Initial observation | Queue wait |
|---|---:|---:|---:|
| AttitudePropulsion | 8.383 s | 3.33 ms | 0.014 ms |
| MainPropulsion | 6.631 s | 7.60 ms | 0.032 ms |
| AttitudeActuation | 2.968 s | 0.57 ms | 0.015 ms |
| ElectricalPower | 0.276 s | 0.42 ms | 0.015 ms |

The heavy member sessions overlap; summing them as sequential elapsed time is incorrect. The regular run compiled the synthetic combined DAE in 2.23 s (2.497 s on the compiler actor). In the instrumented warm run, all 16 compiled-artifact lookups together took 0.444 s, but the coupled observation still took 41.416 s by the inclusive Tracy zone. Thus the DAE disk cache is necessary but not sufficient. No evidence supports increasing worker count as the first fix: the large constructors already occupy separate lanes and queue waits are tiny.

## Native Tracy attribution

The version-matched `../tracy/capture/build/tracy-capture-0.14.1` captured CPU zones at 1 kHz. `scripts/perf/tracy_samples.cpp`'s reader selected 31,169 leaf samples inside the one `modelica_coupled_initialization_observe` zone and 30,793 inside the 15 `modelica_live_session_construct` zones, matching the owned PID 3829058. The symbolized capture uses the retained executable's ELF program headers, binary symbols and its native load map. The concurrently written JIT perf map contains malformed lines; JIT address names were **not** used for attribution. Unresolved samples are retained in the denominators. The following aggregate categories use the top 60 leaf-symbol bins and are lower bounds, not percentages of elapsed wall time.

| Leaf CPU sample category | Coupled observation | Live session construction |
|---|---:|---:|
| Rust string/debug/numeric formatting | 34.05% | 37.29% |
| Cranelift / regalloc2 | 13.79% | 13.36% |
| Unresolved libc | 15.55% | 16.63% |
| `String::write_str` alone (included in formatting) | 13.27% | 14.14% |
| `DebugStruct::field` alone (included in formatting) | 5.48% | 5.99% |

Other visible costs include `CausalDefinitions::derive` (2.06% of coupled leaf samples), scalar program output-position and register dependency projection, memory allocation/free, and register validation. The trace's partially resolved stacks connect formatting to `LinearOp::Debug`, nested `Vec<LinearOp>::Debug` and `rumoca_ir_solve::linear_op::shared_values::symbolic::value_shape`: 46 coupled and 33 live samples have a formatting leaf and an observed `value_shape` frame. Unwinding is incomplete, so these counts demonstrate the path, not its complete inclusive cost.

At the pinned Rumoca source, `crates/rumoca-ir-solve/src/linear_op/shared_values/symbolic.rs:116,145` allocates `format!("{op:?}")` or `format!("{normalized:?}")` as a structural shape key. `Terms::shapes` is a `BTreeMap<String, usize>`. Large nested operations serialize structural dependency expressions repeatedly to text, even on successful preparation; this is not log rendering or Modelica equation evaluation. This is the first concrete optimization target in the compiler owner.

## Optimization candidates from the baseline trace

1. **Remove string-shaped operation keys in Rumoca.** Derive exact typed normalized operation keys over the operation discriminant, operands, constants and nested structural attributes. Intern immutable shapes once where possible and reuse them in both shared-value derivation and proof checking. Preserve float-bit identity, complete structural equality, deterministic term issuance and all collision/validation guarantees. Profile `value_shape`, the term table and proof validation separately; do not suppress proofs to improve a timer. The samples indicate a large opportunity but cannot establish the achievable speedup without the implemented A/B result.
2. **Measure and avoid unnecessary JIT for one-shot initialization.** `lunco-modelica-worker/src/worker/initialization.rs::observe_initialization` constructs a complete `LiveStepper` through `lunco-modelica-solver/src/simulation_session.rs::live_for_solver`, obtains selected initial values and drops the temporary session. Its native path uses the regular solver options, while the Wasm owner explicitly selects interpreter execution. Compare native against an initialization-only/checked interpreter path for this *one-shot* operation, with the same equations, initial states, samples, tolerances and failure semantics. Do not change the runtime solver used for subsequent physics ticks. Rumoca's RK session already has `new_with_stage_timing` for structural DAE, SolveIR lowering, override application and backend setup; expose/consume equivalent timings through the authoritative session API rather than inventing a duplicate solver path.
3. **Retain immutable prepared computation, not numerical state.** The checked compiled-DAE cache currently leaves DAE-to-SolveIR and execution-backend preparation on every `SimulationSession::new_with_diagnostics` call. Investigate upstream prepared artifacts and native executable cache keyed by the complete DAE/source identity, structural parameters, backend/ABI/target and preparation configuration. Any parameter, input and state-sensitive portion must be recomputed against the admitted instance; reject stale artifacts. Do not reuse coupled initialization results across scene generations or allow cache state to change accepted numerical outcomes.
4. **Overlap independent preparation with coupled initialization.** `lunco-usd-sim-cosim/src/initialization.rs::complete` publishes `ConsistentInitialInputs` and state values only after the combined result is checked against the current plan. This intentionally gates member instantiation. Independently preparing immutable member structure/JIT while the coupled solve runs could remove part of the 8.40 s tail. Apply the certificate only at the existing lifecycle boundary; do not begin integration, use provisional values, or weaken invalidation. A cached/prepared API at the upstream solver owner is the right seam.
5. **Only then optimize mount/presentation latency.** The initial ~3 s covers several overlapping CPU and asset activities. Compare headless and a private native desktop on the same binary and exact Twin/cache cohort once a private display is available. Keep terrain and visual quality unchanged. Report physics readiness separately from first fully rendered scene, and keep the UI responsive throughout preparation.

The first proposed target is below 30 s for the measured release workload, then below 20 s if immutable preparation reuse and overlap prove feasible. These are experiment targets, not measured improvements. The follow-up experiment above evaluates the first two changes.

## Reproduction and evidence

The original runs used the following paths under the ignored `target/perf/` directory:

- `griffin-loading-headless/session.json` and `runtime.log` — regular release receipt and phase timestamps.
- `griffin-loading-warm/session.json` and `runtime.log` — same-binary warm, contention-affected repeat.
- `griffin-loading-tracy/startup.tracy`, `modelica-zones.csv`, `session.json` — first attribution capture.
- `griffin-loading-tracy-symbols/startup.tracy`, `modelica-zones.csv`, `coupled-samples.csv`, `live-samples.csv`, `*-symbol-summary.txt`, `process-maps-final.txt`, `binary-elf.txt`, `binary-symbols.txt.gz`, `resolve-leaf-samples.py`, `session.json` — second native capture and samples.

Before calling an optimization accepted, repeat the regular exact-scene startup run, interleave cache cohorts, inspect `GetReadiness`/`CosimStatus` and `last_coupled_initialization`, run authored consistent and inconsistent initialization gates and Griffin's mission handoff, compare state and parameter snapshots and source hashes across separate processes/Twins, and retire each owned session. Report native desktop readiness/images separately; profiling overhead and concurrent builds are not acceptance measurements.
