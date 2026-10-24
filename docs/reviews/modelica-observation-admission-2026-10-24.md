# Modelica observation admission — 2026-10-24

`lunco-modelica-runtime::ModelicaVariables` owns the landed observable map.
It uses Bevy's reflected `HashMap`; every consumer still reads this same map.
Worker admission reuses existing keys, preserves f64 bits and detected-symbol /
step-output precedence, and commits the clock at the existing serial boundary.
Regular development builds optimize `lunco-modelica-worker` at level 3 with
assertions enabled. Connection ordering still belongs to the authoritative wire
comparator; observable hash-table iteration does not select causal write order.

The API projects borrowed entries through its existing name filter, accepting
both parameter/input maps and the observable store. Ready participant publication
collects directly into its existing scalar-port map. There is no additional
state owner, cache, policy hook, dependency patch or duplicate observation path.
Rumoca still supplies the complete checked observation; no solver, communication
period, precision, physics substeps or rendering quality changes.

The manual owner benchmark admits 512 steps with 2304 named observables,
an input and a parameter. Exact final observable bits, filtered cardinality and
the final communication clock are checked. It takes 138.954 ms before and
96.264 ms after, a 30.7% reduction for the combined storage/profile change.
This includes the fixture's named-result construction and does not isolate the
hasher from compiler optimization or establish whole-tick performance.

Seven bridge seam checks pass, retaining signed-zero values, stable key/buffer
ownership, reset recovery, missing-state rejection and stale session/generation
fencing. The regular production `modelica_variable_projection` scene passes all
seven checks. `modelica_variables_missing` produces its expected terminal
`modelica-variables-missing` fault without a process panic. The graphical
compiled-metadata scenario passes all 35 checks: reflected/public values,
filtered API reads, reset, unchanged facts through draft edits, recompilation
and scratch participant retirement. Its final notification was inspected.

The regular production rebuild completes in 3m12s. SHA-256 is
`e813f85b05546ebde1e6d788a0f44e31080ef45d63cedd27f0454acd60b53264`.
It retains the existing unrelated `UsdStageProjectionPlan` unused-import warning.
Fresh graphical runs use the same admitted authored source hashes as
[the preceding three-Twin cohort](scalar-port-exchange-2026-10-24.md).
Qualification requires the intended integrated body, complete terrain residency
and derived maps, five seconds of sustained readiness and a ten-second measured
window. Each coupled-tick summary contains 240 complete samples.
Admission times start at `OpenTwin` after an empty bootstrap scene is ready;
they are not process-launch times.

| Twin | Fully-ready time | Coupled tick p50 / p95 | Frame p50 / p95 |
| --- | ---: | ---: | ---: |
| Summer Space School | 3.875 s | 3.759 / 4.281 ms | 5.336 / 9.929 ms |
| Griffin / lunar-base-model | 24.174 s | 5.747 / 7.662 ms | 13.681 / 17.103 ms |
| Tesla Roadster | 1.802 s | 1.572 / 1.865 ms | 3.618 / 5.887 ms |

The first Griffin attempt loses an admission condition after two measured
samples while shared readiness remains clear. That window is excluded. Its
receipt lacks the other condition facts at the rejection edge, so the specific
cause is unresolved. A fresh owned run adds those rejection facts, completes
the same strict window and does not reproduce the loss. No application retry
or readiness change is introduced. Its touchdown and handoff remain at ticks
1339 and 1340, matching the preceding cohort.

All valid runs retain stable sources, zero diagnostics and no script errors.
Final images were inspected. These are bounded startup/settled observations,
not complete mission or exact native-state replay. The Tesla HUD's unavailable
frame/datum facts remain outside this qualification. Another worktree's Tesla
session, PID 8277, remains active, so contention applies. All original loading,
coupled-tick and graphics targets remain open; these runs do not establish a
causal whole-scene gain or clean graphics acceptance.

The separate regular Tracy build also completes in 3m12s with the same existing
warning. Its SHA-256 is
`7a8a2fa4afb2ae62a6125907b52ad4e822a1a2b240319ea4cd0e5bb0b249deda`.
The Griffin capture waits for the landing handoff before its soak and measurement.
Native Tracy 0.14.1 readback verifies owned PID 773946, 1 kHz sampling, 17,610
compressed blocks and a 342.96 MiB payload, followed by successful native export.
Its source hashes match the preceding landed capture. The final snapshot retains
21 sleeping bodies, 638 bodies, 777 colliders, 31 joints and no orphan graph pairs.
Its final image and diagnostics are checked.

The last 240 complete FixedMain instances cover 58,791,436,578 through
62,688,440,782 ns. Inclusive native owner medians are:

| Instrumented owner | Before | After |
| --- | ---: | ---: |
| Modelica response handling, per invocation | 1.194 ms | 0.660 ms |
| Complete propagation | 1.589 ms | 1.066 ms |
| Native Avian schedule | 1.872 ms | 1.468 ms |
| Complete coupled tick, API | 7.647 ms | 5.328 ms |

Response handling has 450/478 complete invocations in the respective windows;
its median is 44.7% lower. Parent and child costs must not be summed. Unmodified
render and physics owners also change in this comparison, so system workload,
cache and scheduling effects remain limits on attributing the whole-run change
to the hasher or compiler profile independently. Preparation/construction,
native physics, exchange and celestial sampling still exceed the remaining
budget; the full objective is not achieved.

Evidence remains ignored under `.cache/codex-evidence/performance-20261023/`:
`modelica-observation-*`, `{summer,tesla}-modelica-observation-normal/` and
`griffin-modelica-observation-normal-diagnostic/`. The excluded Griffin receipt
is retained separately. Production builds precede the final comment/format/review
boundary; the implementation diff and its hash are recorded beside the receipts.
No profiling data is committed.
The matching capture and exports use `griffin-modelica-observation-landed/` and
the ignored `.tracy` file. All owned gate, API, regular-run and profiler processes
exit successfully; API ports 5122–5128 and profiler port 8417 are released.
