# Griffin loading profile — 2026-11-26

The updated release reached first physics readiness in **15.01 and 16.33 s**
on two warm headless loads, compared with **46.07 and 34.34 s** in alternating
control runs. With an empty Modelica cache, the control took **33.67 s** and the
updated release took **16.80 s**. All measurements opened the exact surface
operations scene.

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
backend. In the baseline Tracy rerun, 55,645 leaf samples fell within the coupled
observation and 49,967 within live construction. Among the top 80 leaf symbols,
formatting accounted for at least 23.6% and 31.4% of the respective samples.
Cranelift and register allocation accounted for at least 22.4% and 17.9%.

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
The fresh baseline CPU capture and zone-filtered sample summaries are under
`target/perf/griffin-repeat-tracy/`.

## Workload and controls

Production `luncosim`, release, application `a2adc5636df87d03eb569fcdca3810bd9952127c`, Rumoca fork pin `2a3716da596e18598e21a3683b1ff6d13ee650a0`, model repository `269273031bec2906b7e6e2ecf15f6d818f45f750`. The retained binaries were built in the adjacent optimization checkout at the same application revision and copied into this checkout for the measurements. The uninstrumented SHA-256 is `3fc5fd5fb15491e3bc8b35064c5fab011d3746ca5209fcf55f6bc4a037017d3a`; Tracy SHA-256 is `e742479de81c107cb01c32e512779a1f49d0783b7562617b95999912ddf3265a`.

`scripts/perf/twin_startup.py --startup-only --headless --scene scenes/griffin_1_surface_ops.usda` used `OpenFile` on the owning `astrobotic-griffin-1` Twin, after a verified ready bootstrap scene. It observed `/Griffin1SurfaceOps/Lander` and the readiness gate atomically in one World snapshot. The application bootstrap took 0.643 s; the timed interval starts at the authorized scene-open command and ends at first physics readiness. The four runs each observed 15 running and six idle co-simulation participants, zero runtime diagnostic errors/warnings, an unchanged 602-file source snapshot, and the owned process and API port retired on exit. Existing asset and library caches were preserved. The first run compiled the Modelica artifacts; the trace runs and regular repeat subsequently hit the compiled-artifact disk cache. These are not a controlled cold-versus-warm timing comparison.

| Run | First physics-ready | Coupled observation | Slowest live solver session |
|---|---:|---:|---:|
| Regular release, API 5497 | 38.114 s | 19.568 s | 8.383 s |
| Tracy, API 5499, profiler 5501 | 89.431 s | 49.734 s | 29.317 s |
| Tracy with native maps, API 5500, profiler 5502 | 77.522 s | 41.044 s | 24.765 s |
| Regular warm release, API 5503 (contended) | 88.935 s | 46.075 s | 31.612 s |

The Tracy runs overlapped compilation workloads and incurred profiler overhead; use them only for attribution, not performance acceptance. The regular warm repeat used the exact original release binary, but the host had unrelated active Rust compilation and browser workloads; its receipt confirms the same critical path despite cache hits. The coupled observation and slowest member construction alone total 77.687 s (87.4% of that run). Do not interpret the warm time as a cache regression. A native desktop comparison failed before application launch because the private Weston compositor was absent; Xvfb is also absent. No physical-desktop timing, screenshot, or visual readiness claim is made.

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
