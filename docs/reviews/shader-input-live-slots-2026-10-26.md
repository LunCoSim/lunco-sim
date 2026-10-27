# Shader input metadata from live slots

## Measured owner

The saved native capture `griffin-native-loading-20261026.tracy`, PID 1090867,
contains 252 complete `cosim_exchange_write` zones in
62,399,939,332–67,399,939,332 ns. Their inclusive total is 256.795 ms, or
1.019 ms per exchange. Native CPU export selects 247 samples, zero missing
stacks and a verified 1,000,000 ns period. Saved ELF mappings and symbols resolve
the application addresses. Leading leaves include resolved-write preparation
(30 samples), ECS component-type lookup (28), scalar-map name lookup (25),
string-set lookup (13) and shader parameter-map lookup (6). Remaining libc
addresses are not attributed to a particular string operation.

This capture contains landing but fails the terrain settled-window condition.
These are diagnostic owner costs, not steady-state product acceptance. The
Modelica worker zones overlap each other on multiple threads and must not be
added as a wall-time explanation of the main-thread tick.

## Change and scope

`lunco-usd-sim-shader::ports` owns the shader backend. Named declaration checks
its driven membership once and publishes the current live slot. Metadata uses
that same slot under the registry's immutable World borrow, reading the current
authored scalar type, or the live type when no authored value exists. Authored
float authority still permits replacing a differently typed live override.
The registry still checks live owner precedence, revision, slot, bounds and
metadata; each transaction remains atomic. No permission cache, extra API,
clock change, dependency patch or Rhai policy is introduced. GPU binding remains
a downstream presentation consumer of the same admitted values.

Twenty independent processes in ABBA blocks run the existing complete
`shader_input_transaction_benchmark`, with 393,216 writes per case. All final
value assertions pass. Median durations:

| Values | Baseline | Candidate | Reduction |
| --- | ---: | ---: | ---: |
| Unchanged | 77.374 ms | 65.587 ms | 15.23% |
| Alternating | 77.548 ms | 66.185 ms | 14.65% |

Eight focused backend checks pass, including unsupported/untyped inputs,
integer precision, authored/live type precedence, slots and no-op change
detection. Fixture gains do not establish a whole coupled-tick gain.

## Initialization execution investigation

Rumoca's explicit interpreter policy is available through its public API. An
isolated comparison uses the same validated prepared component from the current
Griffin initializer, with the application cache owner and normal live numerical
settings. It compares all 6,055 initial observables by name and f64 bits.
The native side records 16,482 cache hits and zero compilations:

| Explicit execution | Construction | Initial observation |
| --- | ---: | ---: |
| Automatic, native selected | 1.890 s | 0.515 ms |
| Interpreter | 7.268 s | 2.755 s |

The bitwise comparison also rejects `p0.minimum_leg_compression` because the
interpreter produces negative zero. That is a representation difference, not
an observed flight failure. The interpreter is already much slower in this
one-shot consumer. No application execution-policy change is retained; the
experimental diagnostic source and extra development dependency are removed.
Native component preparation remains a loading target, and simply omitting an
integrator does not remove it.

Raw source-bound benchmark records, trace exports and the rejected diagnostic
are ignored under `.cache/codex-evidence/performance-20261026/`, in
`shader-input-slots/`, `coupled-write-attribution/` and
`initialization-execution/`.

## Qualified Tracy window and next owner

The candidate's Tracy 0.14.1 run uses PID 1317426, API 5503 and collector port
8459 at 1 kHz. Native framing accounts for all 310,442,457 capture bytes and
the official CSV exporter exits successfully. All 271 source hashes agree
before/admitted/after; Griffin reaches sleeping four-pad handoff, zero registered
warnings/errors and a settled terrain window. The inspected screenshot remains
part of the ignored receipt. Both process and port close.

In the interior 44–49 s capture window, 329 complete write exchanges have
median/p95 0.810/1.140 ms. This is diagnostic, not an isolated production gain
against the earlier capture that failed terrain qualification. Across six
worker threads, 3,948 complete integration zones total 2,067.589 ms; their
overlap prevents treating this sum as a coupled tick. Exact-zone CPU selection
returns 2,077 samples, zero missing stacks and a verified 1 ms sampling period.
Saved executable mappings and symbols identify 150 leaf samples in torn-block
sweeps, 47 in compiled residual calls, 45 in compiled projection assignment
selection and 83 in `_mi_page_malloc_zero`. Unresolved native/JIT addresses and
truncated stacks remain unattributed; allocator leaves do not identify every
allocation's caller.

The critical participant is MainPropulsion: the instrumented receipt reports
6.763 ms mean worker service and 5.711 ms last service, with 9.145 ms last
response latency. Those are different boundaries. `SimulationTimingProfile`
brackets `FixedFirst` through `FixedLast`, excluding asynchronous Modelica work
and completion admission outside that interval. It cannot alone certify the
complete coupled-step target. The separate regular pre-change receipt reports
6.187 ms mean MainPropulsion service despite 3.607 ms median fixed-schedule
service. Neither measurement meets the requested complete sub-ms target.

The current Rumoca pin deliberately excludes mutable torn-sweep workspace
reuse while its [exact replay investigation](physics-first-step-joint-replay-2026-10-25.md)
remains unresolved. Do not reintroduce that patch or disable independent
numerical checks on the strength of a passing landing window. This shader
change does not modify solver execution, native dependencies or replay policy.

## Regular production validation

The regular production build completes with four Cargo jobs and sccache.
Its SHA-256 is `35b1b32910e439f0f18a07199d67f950a8783d462c22b884e5563024c662a779`.
The authored `lander_plume_activity` gate passes all six verdict checks at
tick 242, including canonical shader input discovery, writes, hold release,
undeclared-name and invalid-value rejection. The owned PID 295629/API 5504 exits
successfully. Eight low-level shader checks and the 43-skill catalogue pass.

The normal Griffin run uses PID 305228/API 5505 on October 27. It preserves all
271 source hashes, reaches sleeping four-pad handoff with finite mass/inertia,
remains unpaused and reports zero registered warnings/errors. The screenshot
shows the lander, terrain/shadow and four-pad handoff controls; its closer view
does not establish a matched-view graphics comparison. Process and port close.

Readiness is 27.658 s, fixed-schedule service median/p95 3.591/7.005 ms and
application-frame median/p95 12.899/22.991 ms. The concurrent `main` build is
recorded at launch and qualification. Native cache churn is also explicit:
the initializer performs 16,482 lookups, 4,347 hits, 12,135 compilations and
12,135 evictions; construction takes 13.195 s after a separate 2.675 s prepared
component lookup. This is a contended, partially cold observation, not an
isolated regression or speedup. MainPropulsion worker service averages 7.480 ms
over the run, with 5.027 ms last service and 7.739 ms last response latency.
All three performance targets remain open.

Independent benchmark executables are hashed and removed after validation.
The native capture, compressed zone export, logs, screenshots and receipts stay
ignored. No profiling data or temporary diagnostic source is committed.
