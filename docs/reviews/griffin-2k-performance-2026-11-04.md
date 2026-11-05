# 2K Griffin performance qualification

The target remains 150 FPS at 2560×1440 with visual and simulation quality
preserved, using the canonical `lunar-base-model` Griffin landing. Work is active;
the 150 FPS gate and sustained powered-descent acceptance remain unmet.
Trello is excluded by user request.

## Current compiled bindings qualification — 2026-11-04

The selected normal release is
`510711b5dc3b6803893bcbe07cb7c3a89d6deec92bc062422a00e02e34d969bd`,
independently retained and restored at `target/release/luncosim`. It passes 297
unique authored checks, two expected-fault scenes and 53 focused mechanism/
consumer tests. The final compiler-monitored High/2K run qualifies at 50.2348 FPS
with thirteen participants, 3,035 Main variables, unchanged 528 source hashes and
zero runtime errors. MainPropulsion mean worker service remains 1.7472 ms.

Canonical-ID bindings, retained declaration keys and indexed publication reduce
Tracy response self/publication work to 0.2527/0.0783 ms/tick; live input validation
remains enabled. The admission fixture is 55.54% faster. Product FPS gain is not
established; the 1 ms solver and 150 FPS targets remain unmet. No model, precision,
period or quality changes and no additional dependency edits are introduced.
All owned sessions/ports close; nothing is committed or pushed. See
[the binding review](modelica-slot-bindings-2026-11-04.md) for owners, exact scopes,
cohort limitations, GPU costs and verification receipts.

## Main integration baseline

The user-requested stash/fast-forward/reapply updates optimization from
`dc92300ce` to local main `d6379130f`, which contains fetched `origin/main`.
All 34 dirty/untracked application files are preserved. The new consumer pin
requires Rumoca `5a8dbefb`; the sibling source fast-forwards from `c8fce575`
while preserving all 27 dirty/untracked files. Both backup stashes remain:
application `92257841087613242d68045a55ad326e2d7e6216`, native
`61d4982505d092b27b1ee9138b31dd8af79da1f8`. Conflicts combine initialized
parameter/source-lease contracts with shared observation labels and checked
workspaces; the source tree has no unresolved merge entries. Original unstaged
state is restored. Nothing is committed or pushed.

The integrated SDK initialized-parameter regression and fixed twenty-model
canary pass. All 562,411 values and 16,841 timestamps remain bit-identical,
with twelve strict-high traces and eight unchanged typed refusals. Integrated
fixed-step, worker-admission, fingerprint and binary-wire tests pass. The normal
production app and command-reference generator rebuild successfully. Canonical
dependency pins remain those of main; the separate local lock has exactly 31
source substitutions with unchanged package versions.

The integrated production run passes 294 unique authored checks (46 core,
102 second-pass, 7 observation, 47 clock/receipt, 35 metadata and 57 plume) plus
two expected-fault scenes. The second-pass driver initially fails only after
all assertions pass because its concatenated case-name receipt exceeds the
filesystem filename limit. The canonical writer now uses a bounded SHA-256
selection name; the complete command rerun exits zero and writes its full
receipt. No assertion is weakened. Schema regeneration unions the owned stable
307-command windowed schema with the retained unchanged feature schema: all
14 common definitions match, and all 317 main commands remain documented. No
other agent's live session is used or controlled.

Normal binary SHA-256 is
`ea3c34aaa2ad507636d23b12cc67025ae9bc2991172514ea554123c94808617d`.
A first Griffin take is excluded: pointer selection and a thrust intent are
admitted during measurement, after which terrain streaming readiness changes.
Its process and port close. The fresh owned retry qualifies High/2K, complete
terrain, handoff, advancing physics, unchanged 528 source hashes, 3,035 Main
variables and zero runtime errors. It measures 51.3815 FPS over 526 frames,
p50/p95 19.2577/23.9516 ms. MainPropulsion mean service is 1.7753 ms; aggregate
worker work is 2.8727 ms/tick across thirteen participants, with warm largest-
participant mean median 1.4422 ms. The qualification workload snapshot includes
a concurrent Node health probe; this is no paired or uncontended speedup claim.
No pointer/intent edge is present in the retry log. The final native image is
inspected. All owned ports 5710–5719 are released, canonical lock is restored,
and the normal binary is independently retained. The 150 FPS and 1–2 ms
aggregate-work targets remain unmet. Rendering remains the next major owner.
The measurements below belong to the prior `dc92300ce` cohort and are not a
comparison across integrated physics/lifecycle contracts. Integration evidence
is `.cache/perf/main-sync-tangent-20261104/`.

## Tangent-workspace qualification before main integration

Normal release SHA-256 is
`d8c7002761ba8dd181674677571a606ba3b1ef96e1cdd9c3aa65a5d7091649a6`, built on
`dc92300ce` with the same explicit local Rumoca and clean Bevy overrides. The
native tangent preparation owner shares one retained evaluation workspace across
its projection blocks, preserves exact logical seed extents and clears the
previous plan's registered write ranges before every sweep. Reentry checks out
independent active buffers; Jacobian results remain owned. No equation,
precision, solver setting, cadence or rendering choice changes. The
[native review](../../../rumoca/docs/reviews/torn-tangent-workspace-2026-11-04.md)
records the ownership and lifecycle contract.

Five native boundary tests and evaluator/backend lib/tests Clippy pass. The
fixed twenty-model canary retains twelve strict-high comparisons and eight
unchanged typed refusals, with zero missing/skipped traces and all 562,411 values
plus 16,841 timestamps bit-identical. The compiled-kernel fixture is 36.17%
faster for multi-lane and 23.81% for directional execution. Production passes
111 authored observation, clock/receipt and plume checks plus two expected-fault
scenes. Full solver Clippy still reports only the three preexisting nesting
sites; nothing is suppressed.

The normal baseline/candidate/baseline bracket uses the same 528 source hashes,
High/2K driver and inputs. MainPropulsion mean service is 2.0242 / 1.8018 /
2.2446 ms: 10.99–19.73% lower against the references. Aggregate worker work is
3.0479 / 2.8931 / 3.1588 ms per completed physics tick over thirteen participants,
5.08–8.41% lower. Candidate warm largest-participant mean service has median
1.4350 ms, distinct from aggregate work and barrier latency. No compiler overlap
is observed in guarded windows and no own build overlaps measurement. FPS is
50.53 / 49.19 / 47.68, with p95 22.64 / 24.72 / 26.16 ms. Drifting FPS references
establish no whole-scene gain. Individual worker maxima are 17.15 / 11.39 /
14.59 ms; one bracket does not establish sustained tail improvement.

Candidate peak process RSS is 4.014 GiB versus 4.011/4.041 GiB references. These
are CPU process observations, not GPU usage or a general memory bound. All runs
retain complete terrain, landing handoff, advancing physics, 3,035 Main variables,
zero runtime errors and unchanged sources. The candidate final image is inspected.
Owned apps/ports 5670–5677 close, canonical lock is restored, and the normal
candidate remains at `target/release/luncosim` with an independent archive.
Evidence is `.cache/perf/modelica-tangent-workspace-20261104/`.

The retained post-shared-label Tracy GPU export's final ten-second window has
400 opaque/shadow events: opaque mean 9.4828 ms and early shadow mean 7.9253 ms.
The 7.9222 ms child shadow zone is the same work, not an extra pass to add.
This is diagnostic GPU attribution, not normal FPS acceptance or a claim about
the new workspace's GPU effect. It makes rendering the next major owner for
150 FPS; source and visual quality remain fixed. The 150 FPS, powered-descent
acceptance and 1–2 ms aggregate Modelica-work targets remain unmet.

## Shared-label qualification

After cleanup, normal release and symbol-rich Tracy executables are rebuilt
from integrated main `dc92300ce`. Fresh evidence lives under
`.cache/perf/modelica-after-main-20261104/`; the earlier removed artifact paths
below are historical measurements, not retained files. The normal baseline
SHA-256 is `e3c5457a0d9141a659b1d55f3a337a9b1f8621bc4eb1040de46654faebd6da41`.
Its 100 authored checks and two expected-fault scenes pass. Fresh High/2K landing
windows measure 39.49 and 31.78 FPS, both affected by foreign Rust compilation;
neither is a clean regression or acceptance comparison.

The fresh matching Tracy 0.14.1 capture contains 358,122,810 bytes and passes
framing and native CPU/GPU export checks. Its exact ELF retains 946,600 text
symbols; 5,683 JIT records resolve against 393 captured executable segments,
with no malformed records or ambiguous selected JIT leaves. The final ten-second
window has 601 ten-input worker transactions: mean service 2.1792 ms, integration
1.5890 ms and observation collection 0.5749 ms. Of 856 observation CPU leaves,
431 are allocator work. Unwinding does not recover those allocator callers;
this alone does not assign allocations to a particular source operation.
Input admission averages 6.62 microseconds. These are instrumented attribution
numbers, not clean FPS or coupled-step wall latency.

The checked observation path now retains Rumoca's existing canonical `VarName`
handles through native host, fixed/adaptive session, worker result and accepted
sample. Declared names are interned at host construction and input names at first
admission; steps clone shared handles. The public reflected snapshot still owns
String keys, and external worker names remain text. No new interner, equation,
solver setting, precision conversion or rendering choice is introduced. See the
[observation owner review](modelica-batched-observation-2026-10-24.md).

Fresh collection fixtures give median 36.3211 → 14.5298 ms for 512 reads of
2,305 values, 60.00% lower, across fifteen samples per producer in six alternating
executable runs. Names and every checked numeric bit agree. Nine fixed-step
numerical tests, native input/session tests, worker admission/fingerprint tests,
textual binary-wire checks and the RK45-enabled SDK Clippy gate pass. The fixed
twenty-model canary retains twelve strict-high comparisons, zero missing/skipped
traces and eight unchanged typed refusals. All 562,411 values and 16,841 timestamps
match the fresh baseline bit-for-bit. This is Tier 1 evidence, not the full MSL
cohort. Candidate normal release SHA-256 is
`8ed0f97238ee5292009da37fd24fdca246abce6b83b205980eda1357c8ef937f`.
It passes 100 authored core/observation/clock/receipt checks, two expected-fault
scenes, 35 windowed compiled-metadata checks and 57 plume conservation/invalid-gas
checks. The extra windowed projection verdict repeats the same seven assertions
and is not counted twice. Every owned gate process and port is released.

The normal candidate/baseline/candidate sequence uses the same source cohort,
High/2K driver and explicit independent production executables. Aggregate worker
work medians are 3.3501 / 3.9601 / 4.4802 ms per physics tick over thirteen
participants; MainPropulsion lifetime mean service is 2.2817 / 2.5220 / 2.7487 ms.
The windows measure 50.43 / 46.70 / 25.48 FPS. The first candidate's guarded
measurement window has no observed active compiler; the baseline overlaps a
foreign Cargo operation and the repeat overlaps ten foreign compiler processes.
The 15.4% aggregate-work and 9.5% Main-service differences in the first comparison
are provisional; these cohorts do not establish a confirmed product/FPS gain.
The first candidate maximum is 22.31 ms versus baseline 10.42 ms and repeat
11.95 ms, so no tail improvement is established either.

All three runs retain 3,035 MainPropulsion variables, unchanged model sources,
complete terrain, landing handoff, advancing physics and zero runtime errors.
The first candidate's final native image is inspected. All apps exit through
their own API and release their ports (5637–5639). The 150 FPS and 1–2 ms
aggregate-work gates remain unmet. Collection timing is not a complete-worker
or coupled-tick result. Retained evidence, workload samples and exact binary
identities are under `.cache/perf/modelica-shared-labels-20261104/`.

The candidate's fresh Tracy capture is 376,537,661 bytes, with verified native
framing and CPU/GPU exports. Diagnostic ELF SHA-256 is
`22432ae90df3d81f55e7c6d0cfa07d57dff971c633cad8cc1422687e067440c6`, retaining
946,574 text symbols. All 5,683 JIT map records are valid; selected samples have
zero missing stacks and zero ambiguous JIT leaves at a verified 1 ms period.
In its final ten seconds, 596 ten-input jobs average 1.9812 ms: integration
1.6250 ms and observation 0.3421 ms. Against the prior diagnostic cohort,
observation mean falls 40.5%; integration remains the larger cost. Observation
allocator leaves fall from 431/856 to 16/459 samples. This is diagnostic evidence,
not a confirmed normal/FPS percentage. Integration retains 1,573 selected samples,
including 221 allocator leaves, 34 in selected-output evaluation and 27 in tree-map
entry insertion. Most allocator caller stacks remain unavailable, so their
source ownership is not inferred. All capture processes/ports (5640/8098) close;
the tested normal candidate is restored atomically from its independent archive.

## Selected-output grouping qualification

The next local Rumoca change reuses `SolveRuntime` request scratch to group
selected program outputs, preserving ascending program order and original
within-program request order. It caches no computed values and validates the
whole selection before evaluation. The owner and lifecycle contract are in the
[native review](../../../rumoca/docs/reviews/selected-output-workspace-2026-11-04.md).
Fresh independent producer fixtures reduce ordered median execution time by
32.48% and reversed requests by 23.56%. All sixteen visibility-module tests pass.
The fixed twenty-model canary again retains twelve strict-high comparisons and
eight unchanged typed refusals, with all 562,411 values and 16,841 timestamps
bit-identical to the retained shared-label baseline. Full solver Clippy still
reports the three known nesting violations in unchanged files; no new diagnostic
is introduced or suppressed. Production normal SHA-256 is
`08ce42b0ce88cfd075af5d38ab44f459ede899f1997220ba18be85a558b93be3`.
It passes 111 authored observation, clock/receipt and plume checks plus two
expected-fault scenes. The pre-grouping shared-label binary is the explicit
reference; the canonical model source hashes agree across the full bracket.

The normal baseline/candidate/baseline sequence has no compiler overlap observed
in guarded measurement windows and no own build overlap. MainPropulsion mean
service is 2.3803 / 2.2384 / 2.4165 ms, a 5.96–7.37% reduction against both
references. Aggregate Modelica work medians are 3.6334 / 3.5216 / 3.6429 ms per
physics tick over thirteen participants, 3.08–3.33% lower. The candidate's warm
health windows have largest per-participant mean service of 1.5891–1.9701 ms,
median 1.9000 ms; this is neither aggregate work nor barrier latency. Main's last service
is 1.5432 ms, while its 11.61 ms maximum exceeds the references' 10.08/10.45 ms;
this does not establish a tail improvement. Frame rates are 49.55 / 46.91 /
45.81 FPS, with p95 26.31 / 28.77 / 29.69 ms. The drifting references establish
no whole-scene FPS gain.

All three runs qualify High/2K, source preservation, complete terrain, landing
handoff, advancing physics, 3,035 MainPropulsion variables and zero runtime
errors. The candidate's final native image is inspected. All owned processes
and ports (5650–5657) close, the canonical lock is restored, and the current
`target/release/luncosim` matches the independently retained candidate. The
150 FPS, powered-descent acceptance and 1–2 ms aggregate-work gates remain unmet.
Evidence is `.cache/perf/modelica-selected-output-workspace-20261104/`.

## Modelica critical path

The current focus is the user's 1–2 ms Modelica work target. Separate aggregate
worker work per physics tick from individual participant service. Canonical
model HEAD is now `92094315`; compare the current source cohort separately from
the earlier `7bbd4936` rendering baseline below. The retained development baseline
run on owned API 5569/PID 4183817 reports MainPropulsion mean service **6.6605
ms**, last **6.3571 ms**, maximum **17.6522 ms**, across 2,275 transactions.
It completes the source-preserved High/2K landing at **45.38 FPS**.

The existing standard release profile (`opt-level=3`, thin LTO, strip enabled)
builds without changing equations, tolerance, precision, integration cadence or
profile settings. Owned API 5570/PID 182099 reports the exact
`/Griffin1SurfaceOps/Lander/MainPropulsion` participant Running with all 3,035
variables: mean service **4.3046 ms**, last **3.6963 ms**, maximum **10.8519 ms**,
across 2,268 transactions. This is about **35% lower mean service** than the
development baseline, and remains above the target. Response latency is a
different scope; its last/maximum are 3.8817/283.5576 ms.

Release binary SHA-256 is
`a763f890ece1395d5fa883f8e7250bbb24827948ed3eaaf7350a5189172158aa`.
Its High/2K qualification covers 495 consecutive frames at **48.1085 FPS**,
median/p95 **20.8011/21.7669 ms**. Terrain and landing handoff qualify, physics
advances, runtime diagnostics are clear, all 528 model hashes remain unchanged,
and the final image is inspected. No Cargo build overlaps this run. The process
exits through its API and the port is released. The 150 FPS gate fails; worker
service improvement is not an equivalent whole-scene FPS gain, nor does a
source-preserved run prove exact cross-profile numerical replay.

The same release binary passes the production initialization/clock/receipt
gate's **47 authored checks** on API 5571. The maintained scene-runner negative
gate passes on API 5572/PID 195651: inconsistent initialization returns exit 1
with `modelica-initialization-failed`, without panic or a fabricated verdict.
Both ports and processes are released.

The earlier native Tracy capture is re-read with the clean adjacent 0.14.1
server source. Exact lane 53/OS TID 3939541 integration samples at 20–40 s retain
6,978 samples with no missing stacks and a verified 1 ms sampling period.
Native module identities and 19 independently matched mimalloc symbol ranges
establish the original ELF's load bias; all 1,065 selected static offsets resolve
against that exact verified ELF. Anonymous JIT frames can truncate callers, so
inclusive counts are incomplete. The reference sweep agreement guard alone is
the leaf of 964 samples; compiled assignment schedules, reduced tangents and
native math remain prominent. The guard replays the per-row sweep and compares
every solver-Y/residual bit in debug builds. Standard release omits this
debug-only replay through its existing profile.

The local Rumoca guard retains that replay and replaces its short-circuit
per-element comparison with an integer XOR/OR reduction, allowing vectorization
while preserving all f64 bits, including signed zero and NaN payloads. One
focused native test passes across empty/unequal lengths, vector tails, every
mutated position, infinities, subnormals and NaN patterns. An alternating isolated
3,035-element comparator probe gives median **3.96×** throughput; this is a
comparator result, not a measured solver speedup. No numerical capability,
convergence rule or release execution path changes. Full MSL parity is not
measured by this focused test.

The standard-release Tracy run on owned API 5573/PID 212955 completes cleanly.
Its 92,098,564-byte capture has 92,078,682 payload bytes, a six-byte header and
4,969 four-byte block lengths; native export succeeds. At 20–40 s, the slow
count-10 cohort on lane 50/OS TID 213011 has 1,139 transactions, mean job
5.5686 ms, integration 4.5433 ms, admission 0.2614 ms and observation collection
0.7542 ms. This cohort is selected by lane and input count; the exact public
MainPropulsion status remains the authoritative model identity/timing evidence.
Native math is the leaf of 2,079 of its 5,170 integration CPU samples. These
instrumented timings are diagnostic, not product FPS.

A bounded argument diagnostic reveals repeated exact `pow` inputs. The native
math owner now retains finite results in a 4,096-entry thread-local table with
full f64 input-bit keys. Collisions replace entries; non-finite inputs/results
keep ordinary math evaluation. The existing typed function `InputResults`
remains separate: scalar math imports have no Modelica function-owner coordinate.
No equations, convergence checks, precision or integration settings change.
The native design and verification record is
[`native-power-reuse-2026-11-04.md`](../../../rumoca/docs/reviews/native-power-reuse-2026-11-04.md).

Normal release candidate SHA-256 is
`6f39553f2e955105e81e27e165a41a338b4c1755d9bba698ea97171a457fe684`.
An A1/B/A2 comparison reports MainPropulsion mean service **4.3046 / 3.8224 /
4.1393 ms**, across 2,268 / 2,249 / 2,260 transactions. The source optimization
is **7.7–11.2% lower mean worker service** against the two baselines. Scene FPS
is **48.11 / 48.66 / 48.68**; no scene-level gain is established. No Cargo build
overlaps these performance windows. All runs qualify handoff, terrain, High/2K,
advancing physics, complete variables and unchanged model source, then release
their processes and ports. Candidate final imagery is inspected.

The actual native cache module gives median **2.84×** throughput on the recorded
math stream after complete-bit checks, across eight alternating pairs. The two
focused native cache tests and owner lib/tests Clippy pass. Both application
binaries pass the **17-check radiative-exchange** production gate; 400 observable
values and 100 timestamps match bit-for-bit across the four complete trajectories.
The candidate also passes the **47-check initialization/clock/receipt** gate and
the negative initialization scene-runner gate. Full-Griffin exact replay and
full-MSL parity are not established by these focused results. The fixed MSL
canary runs separately against MSL 4.1.0/OpenModelica 1.27.1: baseline and
candidate compile fourteen targets and compare the same twelve simulated
targets strictly high, with no deviations, missing traces or skipped
comparisons. Six ToDAE and two typed solve-IR refusals remain unchanged. All
twenty owner classifications/errors match; 562,411 trace data values and 16,841
timestamps match bit-for-bit. This is a fixed twenty-model delta, not a
full-cohort parity number.

Evidence is in ignored `.cache/perf/modelica-critical-path-20261104/`, including
release build provenance, API receipts, source hashes, images, exact-ELF symbol
resolution, argument prefixes, math probes, A/B/A comparison and thermal bits.

## Complete native function results

The native function-reuse change extends Rumoca's existing `InputResults` owner to
retain bounded interleaved complete input coordinates. A fresh standard-release
Tracy capture resolves the plume station/Mach calculation subtree through
Cranelift's maintained JIT map. Its emitted cache checks and sampled body
offsets locate repeated expensive evaluation. A focused regression reproduces
four evaluations for two alternating input tuples, then passes with two after
the change. The native owner, capacity bounds, exact-bit matching, failure and
relocation contract are recorded in
[`native-input-result-reuse-2026-11-04.md`](../../../rumoca/docs/reviews/native-input-result-reuse-2026-11-04.md).

The function-cache baseline release SHA-256 is
`50868737c46d4b573461dbd8a356130b80b2050042796c8e78c978e040108020`.
The canonical model remains `92094315`; all 528 hashes remain unchanged in
every comparison. Exact MainPropulsion worker receipts are:

| Cohort | Mean | Last | Maximum | Transactions | Scene FPS |
|---|---:|---:|---:|---:|---:|
| Candidate, API 5582/PID 341151 | 2.5190 ms | 2.1944 ms | 32.0503 ms | 2,297 | 48.36 |
| Fresh power-cache baseline, API 5586/PID 348387 | 3.6505 ms | 3.2181 ms | 9.7783 ms | 2,287 | 48.28 |
| Candidate repeat, API 5587/PID 351836 | 2.3322 ms | 1.9017 ms | 9.2978 ms | 2,279 | 48.51 |

Mean worker service is 31.0–36.1% lower than the fresh baseline. Every run
qualifies High/2K, terrain, landing handoff, advancing physics and all 3,035
MainPropulsion variables without a Modelica error. No Cargo build overlaps
these performance windows. Candidate final imagery is inspected, and all owned
processes and ports are released. The 32 ms maximum is not reproduced and
remains unexplained; no tail-latency or whole-scene FPS improvement is claimed.
The 1–2 ms mean worker and 150 FPS targets remain unmet.

A subsequent cache-composition comparison gives 2.3714 ms mean without
scalar power reuse (API 5588, 2,281 transactions) and 2.4336 ms for a fresh
repeat of the selected implementation (API 5589, 2,269 transactions). Both
remain within the observed repeat range. Removing scalar reuse does not
establish an improvement, so the qualified implementation and normal binary
remain selected. Both runs preserve source hashes and release their processes
and ports; ignored evidence is in
`.cache/perf/modelica-cache-composition-20261104/`.

Nine focused native tests and owner lib/tests Clippy pass. The production
plume gate passes **52 conservation/optical checks**, followed by **five
invalid-gas/retained-sample checks** in the same session. The **47-check
initialization/clock/receipt** gate and negative initialization-fault gate also
pass. The fixed twenty-model MSL/OpenModelica canary retains the same twelve
strict-high comparisons, six ToDAE refusals and two typed solve-IR refusals,
with no deviation, missing trace or skipped comparison. All twenty owner
classifications/errors match, and all 562,411 trace data values plus 16,841
timestamps match bit-for-bit against the unchanged power-cache baseline. This
is a focused canary delta, not full-cohort parity or complete-Griffin exact replay.

Ignored evidence is in `.cache/perf/modelica-jit-critical-path-20261104/`.
The valid capture uses owned PID 318857/API 5581 and matching Tracy 0.14.1;
native export, 1 ms sampling period and file framing validate. Its JIT map has
5,683 complete rows. Current executable mappings bound attribution, with no
ambiguous matching sample labels. The diagnostic serializes writes to that
owned map after concurrent formatting is observed to corrupt the initial map;
normal performance measurements have no interposer or profiling flags.

A separate post-change Tracy capture (API 5590/PID 394340) validates native
export, 1 ms sampling and complete file framing. Code-arena reuse leaves
overlapping map ranges; none of the selected integration sample addresses has
multiple labels. In the 20–40 second count-10 MainPropulsion cohort, 1,157 transactions
average 2.4516 ms integration, 0.7519 ms observation construction and 3.4291 ms
complete instrumented worker service. The slow lane's 2,849 integration samples
contain no PC in the current Mach inversion helper; the free-jet helper retains
172 inclusive samples. This supports the intended reuse effect without proving
zero execution or a further product gain. Observation construction remains
measurable; stripped static symbols limit finer attribution. Evidence is in
`.cache/perf/modelica-cache-composition-20261104/post-cache-tracy/`; the app and
port are released and the qualified normal binary is restored.

## Direct complete observations

The Modelica-only normal release for this stage is
`d7651794043acafd2a76f69dcaf3e3f50ec4f7fa444745cdb27ea6becd71e808`.
Rumoca's common ME host evaluates and checks the complete observation, applies
input overlays, then visits borrowed names and f64 values. Adaptive and fixed
LunCoSim sessions collect owned rows directly; finite-value validation and
publication remain atomic. This removes the worker's temporary `IndexMap`.
Input output positions resolve on first admission and retire with the input on
reset. See the [observation owner contract](modelica-batched-observation-2026-10-24.md).

A separate symbol-preserving standard-release Tracy capture, API 5591/PID
409999, resolves all 170 selected static observation PCs against the exact ELF.
It identifies SipHash/map construction and allocation alongside native value
validation. This pre-change diagnostic uses 1 ms samples, matching Tracy
0.14.1, valid native export and complete framing. Its app exits cleanly.
The focused release collection diagnostic passes and reduces median time for
512 reads of 2,305 values from 63.2049 to 34.1886 ms (45.91%); this is not a
whole-worker measurement.

Fresh normal High/2560×1440 Griffin runs use the same enhanced measurement
driver, canonical model `92094315`, and identical 528 source hashes:

| Cohort | MainPropulsion mean service | Aggregate Modelica median ms/physics tick | Participants | FPS |
|---|---:|---:|---:|---:|
| Direct rows, API 5598/PID 465848 | 2.2108 ms | 3.6340 | 13 | 51.37 |
| Named-map baseline, API 5599/PID 466781 | 2.3918 ms | 4.0543 | 13 | 48.89 |
| Direct rows repeat, API 5600/PID 468350 | 2.2304 ms | 3.7298 | 13 | 51.12 |

Aggregate worker work is 8.0–10.4% lower; MainPropulsion mean service is
6.8–7.6% lower. Aggregate work sums overlapping worker contributions and divides
by the owner's completed physics-tick denominator; it is not barrier or
coupled-tick wall time. The largest participant's settled median is 1.8317 ms
in the first candidate, while aggregate Modelica remains above 1–2 ms. The
150 FPS gate still fails, and GPU-clock/whole-scene causality is not established
by these comparisons. No build overlaps the windows. All runs qualify landing,
terrain, advancing physics, 3,035 MainPropulsion variables and clear runtime
diagnostics. The candidate's final image is inspected; owned apps and ports exit.

Native input/session tests pass, including hidden inputs, signed zero, reset
cleanup and no visitor calls after a failed session. The `rumoca-sim` lib/tests
RK45-enabled Clippy gate passes. A combined owner Clippy run is limited by
three pre-existing excessive-nesting errors in untouched `relation_memory.rs`
and `tangent_evaluators.rs`; no lint suppression is added. Production variable
projection passes seven checks, and missing-state/non-finite-observation scene
gates pass with their expected faults. The 47 clock/receipt checks, 35 desktop
compiled-metadata checks, and 52 plume conservation plus five invalid-gas checks
pass. An attempted editor-metadata run in the headless host times out before
compilation; the unchanged scenario passes in its windowed command owner.
The fixed twenty-model MSL canary retains twelve strict-high comparisons, zero
missing/skipped comparisons and eight unchanged typed refusals. All 562,411
data values and 16,841 timestamps match the previous canary bit-for-bit.

Evidence is in `.cache/perf/modelica-observation-owner-20261104/`. The selected
binary is retained independently. This API consumer requires the local Rumoca
patch override; the published dependency pin has not advanced. Promote the
native SDK and application consumer together before claiming portable builds.

## Zero-output local-light qualification with current Modelica

The selected normal executable is
`4df412386cbcf916645253ea1854f68649cd81a70b51394a346d2040f4d2772a`.
It combines the checked observation visitor with existing clean Bevy commit
`eabfa346c153337a287c0de0b3513efe6b820e04` through local overrides. No dependency
source is edited or published. Compared with the registry packages, only
`bevy_light/src/cluster/assign.rs` and `bevy_pbr/src/render/light.rs` differ:
exactly zero-intensity local lights leave clustering/extraction and reenter
when changed. Package versions and dependency arrays are unchanged; metadata
confirms one ECS/render/light/PBR dependency universe. Canonical dependency
pins remain unchanged, so the qualified build still requires local overrides.

Four fresh High/2560×1440 runs use canonical model `92094315`, identical 528
source hashes, completed terrain, advancing physics, landing handoff and clear
runtime diagnostics. Each app exits and releases its port. The baseline is the
Modelica-only `d7651794` binary above; no build overlaps a measured window.

| Cohort | Baseline FPS | Candidate FPS | Gain | Candidate p95 frame |
|---|---:|---:|---:|---:|
| Read-only GPU monitoring, APIs 5603/5601 | 48.91 | 51.70 | 5.71% | 20.40 ms |
| Clean confirmation, APIs 5605/5604 | 49.12 | 51.04 | 3.92% | 20.71 ms |

The monitored windows both report 100% GPU utilization, median graphics clocks
2640/2647 MHz and approximately 79.8 W draw on the RTX 5060 Laptop GPU. Observed
draw does not establish a power limit. MainPropulsion mean service is
2.1849/2.2071 ms in that comparison. These paired results support a modest
rendering gain; neither 150 FPS nor 1–2 ms aggregate Modelica work is achieved.

After the clean candidate's measured window, read-only registered-light
inspection finds nineteen plume PointLights with exactly zero intensity,
25 m range and shadows disabled. This is a registered main-world audit, not a
count of all scene lights or extracted render lights. The final scene image is
inspected. Both baseline and candidate pass the vertex-shadow alignment and
shared-shadow allocation fixtures (seven authored checks); their final full
RGB images match byte-for-byte, with zero red probe pixels. This is focused
rendering coverage, not a complete rendering-suite result.

Evidence, exact process identities, dependency/source deltas, independent
executable and source-preservation receipts are in
`.cache/perf/zero-light-current-modelica-20261104/`.

A separate Modelica validation-loop trial hoisted a tracing-level check outside
the row loop. Six alternating release fixture executions measured a 1.75%
median collection improvement with overlapping ranges. The source is restored
byte-for-byte; no product gain is claimed. Evidence is in
`.cache/perf/modelica-validation-batch-20261104/`.

## Latest main integration and evidence retention

At the user's request, `optimization` fast-forwards from `8083f8618` to fetched
`origin/main`/local `main` `dc92300ce4a333ef34258fa7737ecfbf700d2e46`.
The branches have no divergence. All local tracked/untracked source work is
restored without conflicts; the Git preservation stash remains
`c313f15547a8a7cb539e8f598fb6e8d4c72f38b8`. Review confirms that the new checked
parameter-transaction owner, strict command admission, observation visitor and
transport-clock safeguards coexist. Nothing is pushed. The integrated canonical
lock SHA-256 is
`25504512bf166fbdd19aae00706e6c32eb687275825f256bbaa046ebb979f7f1`.

The standard optimized release build passes with the existing explicit local
Rumoca/Bevy overrides. Its executable SHA-256 is
`d25edfb4c21d46850cb9bf63f2d58f1506c809bf2b10d64a5291210224d61cbd`.
Production command admission (18), input contracts (16), parameter replacement
(12), variable projection (7), and initialization/clock/receipt (47) checks pass:
100 authored assertions, plus the missing-state and non-finite-observation
scenes' expected faults. Owned sessions use API 5607–5610 and 5612 and close.
The settled desktop runtime schema has 307 commands, includes all 306 currently
documented names, and adds `ProjectCameraFeedSubject`; regeneration is pending.
The 43-skill catalogue and integrated diff check pass.

The first integrated High/2K Griffin window, API 5613/PID 39953, reaches landing
with completed terrain, advancing physics, no runtime errors and all 528 hashes
of model `92094315` preserved. It measures 24.1084 FPS, with median/p95 frames
35.8941/78.6551 ms, and fails 150 FPS. Its workload snapshot records a concurrent
Cargo build in the `tutorials` checkout. This is contention-affected evidence,
not a clean regression or comparison against the preceding approximately
51 FPS windows. The app exits and releases its port. MainPropulsion mean service
is 3.3922 ms in this affected run; it does not replace a quiet Modelica baseline.

Subsequent cleanup removes this checkout's entire `target/` and `.cache/`,
including the integrated executable, previous independently retained binaries,
raw captures, runtime receipts and ignored analysis helpers described above.
Source edits and the preservation stash survive. Historical numbers in this
review describe observed runs, but the referenced ignored evidence paths are
no longer retained and must not be presented as available artifacts. Rebuild
and collect fresh quiet acceptance/profiling evidence before further performance
claims; neither 150 FPS nor 1–2 ms aggregate Modelica work is established.

## Production baseline

Application source is `8083f8618`; canonical model HEAD is `7bbd4936`. Normal
binary SHA-256 is
`919c74c66b8bbe8bf5b798792b79dcdba35b141cff03737caa501f83f95ccd31`.
Owned API 5544/PID 3960379 reaches `landing_handoff=1`, then measures 500
consecutive frames over the source-preserved settled window. High quality and
the active camera's 2560×1440 target are checked throughout. Terrain is complete,
physics advances, retained runtime diagnostics are empty and the image is
inspected. No build overlaps this run. The process exits and releases its port.

Mean FPS is **48.62**, with median/p95 frame time **20.43/24.95 ms**. The 150 FPS
gate fails. Earlier 10.05 FPS Griffin data overlaps builds and memory pressure;
it is not the clean baseline. Summer data does not substitute for Griffin.

The maintained driver `scripts/perf/twin_startup.py` supports
`--window-size 2560x1440 --min-fps 150`, retains the existing admission and source
checks, rejects incomplete frame history and requires mean FPS plus p95 to meet
the budget. Optional native-window sizing requires Linux X11/XWayland and
`python3-xlib`. Profiling and FPS acceptance use separate processes.

## Tracy attribution

The fresh opt-in Tracy build passes in 12m39s. Its SHA-256 is
`8c8281112b4b7385d7a90e30eb9088ba78d2604378b85158fa8e8c0b6d86248d`.
Adjacent Tracy 0.14.1 collector PID 3939431 starts before owned API 5543/PID
3939466; the exact socket pair is verified. The requested sampling rate is
1 kHz. The capture covers initialization, descent, handoff and a settled window;
all 528 model source hashes remain unchanged and runtime diagnostics are clear.
Both processes and ports are released.

The saved capture is 315,741,730 bytes: 315,683,960 payload bytes plus a six-byte
header and 14,441 four-byte block lengths. Its 301.0597 MiB payload agrees with
the collector's 301.06 MiB report. All five native exports succeed. Exact raw CPU
exports are gzip-archived with verified round-trip SHA-256 values.

The late 53–60 s diagnostic selection has these inclusive medians:

| Owner | Median | Events |
|---|---:|---:|
| GPU opaque 3D | 10.564 ms | 248 |
| GPU camera early shadows | 7.637 ms | 248 |
| Native physics schedule | 1.924 ms | 470 |
| Connection propagation | 1.017 ms | 471 |
| Modelica integration on worker lane 53 | 2.480 ms | 940 |
| Observable collection on worker lane 53 | 0.425 ms | 940 |

These nested and cross-thread values must not be summed into a frame total.
GPU children are contained in their parents; recorded GPU passes do not cover
every operation or gap. Profiling overhead is not product FPS. The current High
preset's 8192-pixel directional shadows remain enabled. The earlier retained
Griffin capture uses a different rendering/source cohort and is not a paired
comparison.

## Native input ownership

Rumoca `MeSimulationSession::record_inputs` updates retained input values through
borrowed-key lookup. New keys allocate once; existing keys retain their storage.
Validation, source-ordered duplicate handling, signed-zero bits, component
writes and numerical-history correlation stay on their existing owners.

The ignored `repeated_input_transaction_diagnostic` measures 4,000 complete
public setter calls per width, checks exact final values and checks the native
component/session coordinate. Twelve alternating pairs use independent executable
copies with distinct, unchanged hashes:

| Inputs per batch | Median complete-transaction improvement |
|---:|---:|
| 1 | 18.58% |
| 8 | 15.82% |
| 32 | 13.46% |
| 128 | 5.82% |

All three focused input-batch tests pass, including atomic rejection, duplicate
names and signed zero. Measured batches reuse the same admitted values; this is
a native development-profile fixture result, not a measurement of changing-input
integration or a Griffin FPS gain. Owner Clippy is blocked by three excessive-nesting
errors in `relation_memory.rs` and `tangent_evaluators.rs`; both files are
byte-identical to baseline HEAD. No reported error points to the changed code.

The experimental production build uses an explicit ignored local Rumoca source
override. All package versions and dependency edges match the baseline lock;
only 29 native source identities change, including Rumoca's existing vendored
Diffsol identity. No Diffsol source changes. Unrelated Windows dependency-edge
changes from the first resolver attempt are rejected. The local override and
lock are experiment inputs, not a portable published dependency pin.

## Clock boundary regression

The first candidate network run fails the 0.25× pacing-rate assertion. Three
additional alternating baseline/candidate pairs all pass, so that first outcome
is retained as an intermittent failure rather than erased by retries.

Source inspection finds a deterministic defect: frozen clock projection replaces
the configured positive rate with 1.0. The strengthened authored network gate
requires paused pacing rate to equal the requested 0.25× rate. The unchanged
baseline fails this new assertion, with `TESTS_FAIL 1/45`.

The time owner validates a complete requested transport before mutation and
retains the positive configured rate during holds. Pacing projection remains
before frame time sampling; the network gate waits for that owner's update
as well as its minimum tick interval before checking the exact pacing rate.
Invalid internal state raises
`simulation-transport-invalid` and pauses the existing clock without inventing
a rate. The unused clamping helper and its obsolete tests are removed. Direct
observer fixtures install the command-result resources required by the current
typed command wrapper. Unchanged projections preserve the pacing resource's
change tick. All ten focused transport tests pass on the frame-boundary revision.
Its production build passes in 2m27s. Three independent production scene-test
runs pass all 45 network assertions. The negative fixture observes the expected
terminal `modelica-initialization-failed` fault. All owned test processes and
ports exit. The same frame-aware test still fails the untouched baseline's
pause-rate assertion, so the production assertion is retained rather than weakened.

The combined normal candidate SHA-256 is
`7a5db464c0596a8fc659ec764f1997be6f4a8adc2b29f2a87baa4cc5c4d8cc7a`.
Owned API 5548/PID 4055856 preserves all 528 model hashes, reaches handoff,
advances physics through the settled High/2K window and has empty retained
diagnostics. Its inspected image retains vehicle, terrain and shadows. The
process exits and releases its port. It records 496 consecutive frames at
48.34 mean FPS, with median/p95 20.65/23.40 ms. This pair establishes no
whole-scene improvement and still fails the 150 FPS budget. The dirty local
native identity disables persistent solver caching; its 14.61 s readiness is
not a like-cache loading comparison with the baseline's 7.08 s. Formatting-only
cleanup follows validation; no equations, tolerances or render settings change.

## Terrain roughness experiment

Combining the three unresolved slope variances before their perceptual-roughness
transfer produces no useful measured GPU improvement. The experiment is reverted;
all three shader sources match their original bytes. The shared noise spectra,
maps, shadows and simulation settings are unchanged.

The live composed terrain binding and shader source are verified before editing.
Owned normal API 5550/PID 4063839 reaches landing handoff with complete terrain,
then pauses for a fixed-pose rendering diagnostic. Two baseline screenshots are
pixel-identical. A differential check of 2,097,152 float32 cases across both
consumers and platform octave budgets finds at most three ULPs of roughness
difference. The candidate changes eight of 3,686,400 rendered pixels by one
8-bit channel level. These are scoped numerical and image checks, not a claim
of equivalence over every view.

Three alternating normal rendering pairs range from 45.96–47.21 FPS for the
baseline and 47.35–47.54 FPS for the candidate. Simulation is paused, so these
numbers do not qualify application performance. A separate capture uses the
previously identified Tracy executable, whose render implementation is unchanged;
it does not qualify the newer clock/native-input changes. Collector PID 4070996
starts before owned API 5551/PID 4070999, with the socket pair verified. The
requested sampling rate is 1 kHz; its saved CPU sampling period is not verified.
The collector exits successfully, file framing agrees with its compressed-size
report, and both native GPU and message exports pass. Logged shader reload
boundaries select two complete baseline/candidate window pairs, excluding reload
and compilation intervals:

| Pair | Baseline opaque mean | Candidate opaque mean |
|---|---:|---:|
| 1 | 10.5384 ms | 10.5309 ms |
| 2 | 10.7163 ms | 10.7130 ms |

The apparent opaque improvement is below 0.1%; early shadow means remain
7.65–7.67 ms. This does not substantiate the normal-window difference or justify
retaining the shader change. Repeated screenshots of each variant are
pixel-identical within that variant. Both sessions preserve the paused tick and
camera, retain empty runtime diagnostics, show no GPU validation failure, and
exit with their API and Tracy ports released. Evidence is under
`.cache/perf/roughness-20261104/`, including the rejected source variant and the
qualified capture/export receipts. The earlier ownership-check attempt exits
before opening the Twin and is not measurement evidence.

## Depth prepass and zero-Sun lookup checks

Owned normal API 5553/PID 4074772 confirms native `DepthPrepass` is already
present on the active 2560×1440 Griffin camera. The source-declared depth owner
retains it for consumers; no camera component change is made.

An application-owned shader experiment evaluates the existing native Sun
radiance before its shadow lookup and skips that lookup only for exactly zero
radiance. The frozen baseline/candidate images are pixel-identical, but three
normal rendering pairs are too variable to establish a gain. A separate
adjacent-Tracy capture uses collector PID 4077658 and owned API 5554/PID
4077688; the receipt and socket pair are retained. The normal app is still
closing when the profiler launch starts, so this is not startup acceptance.
The normal process is verified gone before the later frozen measurement.
The two opaque GPU mean pairs are 11.2650→11.2310 ms and
11.4373→11.4380 ms; early shadows remain 7.70–7.71 ms. All four images are
pixel-identical, but the pass timings do not establish a consistent useful
improvement. The shader is restored byte-for-byte. Capture framing and both
native exports pass; owned processes and ports are closed. Evidence is under
`.cache/perf/depth-prepass-20261104/` and
`.cache/perf/sun-zero-contribution-20261104/`.

The half-angle photometry experiment replaces `tan(acos(cos_phase) / 2)` with
the half-angle identity. An independent float32 check covers
3,145,728 cases including exact phase endpoints, near-endpoint values, the
authored UI ranges, the existing width floor and zero amplitude. Maximum
old/new difference is 4.7684e-7. The normal frozen image changes six pixels by
one 8-bit channel level. In the separate qualified Tracy capture, the opaque
mean pairs are 10.8129→11.0445 ms and 11.1822→11.1910 ms. The candidate does not
improve this workload and is reverted byte-for-byte. Collector PID 4084500,
owned API 5556 and its process retire; native GPU/message exports and capture
framing pass. Evidence is under `.cache/perf/half-angle-20261104/`.

## Native shadow input diagnosis

`SceneCameraAudit` now has an opt-in native shadow read in
`lunco-usd-sim-cosim-api`. It exposes exact main-world cascade matrix bits and
candidate counts/fingerprints through the existing `lunco-hash` substrate.
Raw pose pages are bounded to 64 candidates per map; missing inputs remain
explicit, and default camera audits do not traverse shadows. Native camera
identity, activity and 3D classification distinguish actual view candidates
from 2D UI-camera cascade entries. The API, performance runbook and crate
index document the current contract. The source/lock change adds only the
existing internal hash dependency; third-party pins remain unchanged.

The production baseline fails the missing-data assertion. The bounded-read
baseline fails four of eleven assertions. The current production build passes
39 authored checks covering default omission, native matrices and caster data,
page bounds, compact/page fingerprint agreement, changed light projection,
changed caster pose with fixed projection, invalid parameters and an empty
host. The empty-host callback uses command context because no scene lease
exists for a retained scenario. Native rendering logs have no GPU validation
failure. Test sessions close their owned API port 5560.

The regular production binary SHA-256 is
`a4bb6407f7b0f887b4ad6ccf0d39249767c71466c2e5e467dc1b029909c34e06`.
It includes the local Rumoca input experiment and frame-boundary clock fix;
build receipts retain the explicit native override and canonical lock restore.
The shadow audit changes reads only and establishes no product FPS improvement.

Owned API 5562/PID 4108498 runs canonical Griffin at High/2560×1440, preserving
all 528 model source hashes and reaching landing handoff with complete terrain.
Each phase records 80 snapshots and 79 consecutive native host-frame pairs,
without duplicates, gaps, missing pose/mesh inputs or runtime diagnostics:

| Phase | Simulation ticks | Each active 3D cascade's observed input changes |
|---|---|---|
| Paused | 1328→1328 | No matrix, membership, pose or mesh-identity change in 79 pairs |
| Running | 1410→1535 | Projection and pose fingerprints change in all 79 pairs; membership and mesh identities remain stable |

This gives no admission opportunity for input-equality whole-map reuse in the
measured running landing window. Equal fingerprints in the paused window
remain diagnostic candidacy: shader/material contents, deformation, native
texture allocation and GPU state are not covered. This is explicit query-load
diagnosis, not FPS acceptance. The maintained
`scripts/perf/shadow_input_changes.py` reader rejects sparse polling as
consecutive coverage and excludes non-active/2D camera entries. The UI host's
four additional main-world cascades do not imply GPU maps: native PBR light
preparation selects `Camera3d` views. Filtering unused 2D preparation through
the native public API is a separate candidate, not a proven speedup.

Evidence is under `.cache/perf/shadow-input-audit-20261104/`, including build
receipts, authored verdicts, raw bit/fingerprint snapshots, sampling receipts
and source hashes. The compact collection is 1.7 MiB; the initial full-pose
probe was too sparse to support consecutive-frame conclusions. All owned
apps/ports are closed.

## Native GPU preparation and occlusion trial

The application retains CPU mesh-instance preparation. Enabling Bevy's native
GPU instance builder is slower in the current High/2K Griffin diagnostic, and
adding native camera or Sun occlusion culling establishes no useful gain. Both
application source files are restored byte-for-byte; the restored normal build
passes. No dependency source, authored scene, quality setting or culling default
changes.

The owned normal processes use API 5564/PID 4126852 for CPU preparation and
5565/PID 4133259 for GPU preparation. Both reach handoff, settle terrain and
pause before manually advancing to exactly tick 1600. The frozen camera target
is 2560×1440, diagnostics are empty, and all 528 model source hashes remain
unchanged. These are rendering diagnostics, not running FPS acceptance:

| Native preparation and culling | Mean FPS | p95 frame time |
|---|---:|---:|
| CPU reference | 48.48 | 21.66 ms |
| GPU preparation | 42.31 | 28.44 ms |
| GPU plus camera occlusion | 42.81 | 25.94 ms |
| GPU plus camera and Sun occlusion | 41.02 | 29.58 ms |
| GPU after removing both occlusion components | 41.06 | 26.32 ms |

The camera and Sun components are applied and removed through the existing
typed Rhai component interface. The same-session screenshots differ by at most
one channel level in one pixel across the occlusion toggles; the two original
GPU screenshots are identical. This is limited frozen-image evidence, not
moving-view culling acceptance. CPU and GPU runs have different floating render
origins despite the common simulation tick; their cross-process pixel differences
are not an isolated culling comparison. A busy Chrome renderer is recorded in
the GPU workload snapshot. The temporal variation and sequential runs preclude
an isolated percentage-regression claim, but provide no reason to retain the
candidate.

Direct production `vertex_shadow_alignment` and `shared_shadow_allocation`
fixture invocations pass for both variants: seven authored assertions, three
recorded frames per fixture, green/cyan probe coverage and zero red pixels.
The normal suite runner is blocked before fixture selection by an existing
`six_independent_parity.usda` composition error: duplicate `Motor_L0` in the
referenced `SixWheelIndependent` prim. The direct invocations qualify these two
fixtures only. Griffin logs in both variants also contain unresolved imports
for the authoring-only `griffin_skirt_detail` tool; empty retained diagnostics
do not certify that every tool library loaded.

All owned apps and ports are closed. Ignored evidence is under
`.cache/perf/gpu-prepare-20261104/`, including independent CPU executable
provenance, candidate and restored build logs, fixture receipts, captures,
native component replies and complete frame samples. The GPU candidate binary
SHA-256 is `ab6d149fc78975a81b57e7c4a8bc3b1ac30c787f4a4d205d7a747a0d170062df`;
the restored normal binary is
`e27abb064f60c92bb8b4bbd5cccb6e896a48fcdced216803fbf69283383bf645`.

## Terrain lighting attribution

A separate adjacent-Tracy run removes groups of fragment work temporarily to
locate the opaque-pass cost. These are deliberately altered diagnostic images,
not optimization candidates or quality acceptance. Each variant is bracketed
by the original shader in the same paused process; all five original-shader
images are pixel-identical. The original source is restored byte-for-byte.

Collector PID 4143263 starts before owned API 5566/PID 4143287; the receipt
retains the verified socket pair. The executable is the archived CPU-preparation
Tracy build, SHA-256 `8c8281112b4b7385d7a90e30eb9088ba78d2604378b85158fa8e8c0b6d86248d`,
with the same rendering implementation and older clock/native-input code. Its
use here is limited to frozen rendering attribution. The requested sampling
rate is 1 kHz; the saved sampling period is not independently verified.

Nine windows preserve one active 2560×1440 camera and tick 1327 after handoff
and terrain completion. Retained diagnostics are empty and all 528 model source
hashes are unchanged. Native GPU/message exports and compressed-file framing
pass. Selection excludes the first five seconds after each reload and the final
half-second before the next reload.

| Terrain fragment variant | Opaque GPU mean | Surrounding original means |
|---|---:|---:|
| No procedural bump, dust or detail-roughness filtering | 9.529 ms | 10.268 / 10.436 ms |
| Native PBR without the extra lunar Sun response | 10.471 ms | 10.436 / 10.606 ms |
| Material colour without lighting | 2.975 ms | 10.606 / 10.775 ms |
| Constant terrain colour | 0.711 ms | 10.775 / 10.803 ms |

Camera early shadows remain 7.59–7.66 ms. The removed material computations,
compiler dead-code elimination and altered shading inputs make these ablations
non-additive; their differences are not isolated function costs. They locate a
larger opportunity in terrain lighting than in the previously tried arithmetic
changes. The shadow pass also needs improvement; the constant-colour diagnostic
does not remove its work.

GPU power samples record substantial clock variation: original windows span
roughly 2.44–2.72 GHz and 76–88 °C; the material-only and constant-colour windows
run at lower clocks. One live limiter probe reports software power capping.
The original opaque means drift upward by about 5%, so sub-ms differences must
not be treated as precise savings. No clock or power setting is changed.
Evidence is under `.cache/perf/terrain-attribution-20261104/`.

### Local-light and clustering follow-up

Owned normal API 5567/PID 4147436 finds 19 native plume point lights after
cutoff, each with exactly zero intensity, 25 m range and shadow maps disabled.
A diagnostic reduces only their live render-component ranges to 0.1 m, then
restores 25 m. No authored layer or simulation value changes. Both reduced-range
images and both restored images are pixel-identical to the original frozen
image. The normal frozen windows measure 48.96 FPS initially, then
50.63 / 46.46 / 49.49 / 46.09 FPS for reduced/original/reduced/original ranges.
This establishes avoidable zero-output light work as a candidate, not a
qualified percentage speedup or permission to retain altered ranges. Every
light's original range is read back before shutdown. The first restore command
rejects before mutation because its guard compares reflected native f32 0.1
with a different f64 value; the diagnostic guard is corrected to the explicit
native f32 representation, without relaxing equality. Evidence is under
`.cache/perf/cluster-range-attribution-20261104/`.

An application-only trial selects Bevy's supported CPU light clustering through
`GlobalClusterSettings`, independently of mesh-instance preparation. Its normal
binary SHA-256 is `2cc5ea18a02fd9ac33134bb9b5e061ae1a0875ff17382687f6a3b179067cccb6`.
Both direct production rendering fixtures pass their seven authored assertions
and pixel probes. Owned API 5568/PID 4152586 reaches the same tick-1600 High/2K
diagnostic and measures 48.36 FPS, which establishes no useful gain over the
GPU-clustering references. The application source is restored byte-for-byte;
this clustering choice is not retained.

Within that process, disabling only the terrain fragment's native shadow-receiver
bit measures 48.93 / 46.32 / 49.61 / 47.17 FPS for disabled/original/disabled/original
windows. Each disabled image changes 212,449 pixels; both original images are
pixel-identical to the initial capture. This is another diagnostic, not an
acceptable quality tradeoff. The original shader is restored. Source hashes
remain unchanged, retained diagnostics are empty, and all owned apps and ports
are closed. Evidence is under `.cache/perf/cpu-light-clustering-20261104/`.

## Evidence and remaining work

Ignored evidence is under `.cache/perf/fps-150-20261103/`: session receipts and
images, build/test logs, the native capture/exports, fixture comparisons, exact
source-substitution records and independently verified executable archives.
The user-authorized old-binary cleanup recovers 17,476,938,680 bytes; the latest
`luncosim-main-integrated-14` stays unchanged. `cargo clean -p lunco-luncosim`
removes inactive package outputs only after executable archives are verified.
Source and shared caches are preserved.

The canonical application manifest and lock retain published Git pins; the local
native optimization remains an explicit experiment until a portable pin is
reviewed. The exact candidate lock and override are retained with the build
manifest for reproduction. A temporary ignored override also builds the already
existing clean local Bevy `eabfa346` zero-output-light implementation without
editing its source or changing canonical dependency pins. Direct vertex-shadow
and allocation fixtures pass seven authored checks and three image/probe checks;
their images match the CPU baseline exactly. The candidate Griffin comparison
is deferred for the user's Modelica focus. Publication of dependency patches is
not authorized. No commits, merge or push are made.

Modelica remains the active optimization owner. Clean 150 FPS evidence must
cover powered descent as well as the settled landing window.
