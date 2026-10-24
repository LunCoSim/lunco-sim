# Ordered Modelica observation publication

Owner: `lunco-modelica-runtime::ModelicaVariables`. The worker's serial result
admission publishes native f64 observations; reflection, telemetry, canvas, API
and co-simulation consume that same snapshot. The map uses maintained native
`IndexMap` with Bevy's fixed hasher and built-in map reflection. The workspace
already uses both libraries; no dependency fork or custom reflected container
is introduced.

Each detected-symbol and output batch aligns the map's indices to its observed
names. Indexed updates check the actual key first. Reordered and new rows use
native named lookup and index swaps. Duplicate rows, including more rows than
distinct names, retain last-write semantics; omissions retain earlier values.
Input/parameter exclusion checks their live maps on every admission. Outputs
retain precedence over detected symbols, with exact f64 comparisons and existing
change detection. Compile/reset clearing, session fences, terminal faults and
accepted transaction ownership remain at their existing owners. No secondary
cache, classification index or policy hook is added: this is engine dataflow.

The baseline starts at application `a78c6c780`. The existing diagnostic exercises
512 real result-admission transactions with 2,304 variables, one input and one
parameter, including result creation. Five alternating pairs measure median
78.453 ms baseline and 66.599 ms candidate, 15.1 percent less time. Every run
retains final bits `4080000000000000` and the exact communication clock. This
bounded consumer measurement does not establish a whole-tick speedup.

Seven focused worker checks pass. The storage/transaction seam additionally
checks reordered rows, omissions, duplicate counts beyond the map width and
changed live input classification. Existing key pointers, transferred buffers,
signed zero, reset isolation and stale results remain covered.

The latest source-matched baseline capture's settled window has 478 response
handler invocations, 0.615/0.811 ms median/p95 and 292.485 ms total. Its 240
complete FixedMain zones have 4.248 ms median. These inclusive handler times
can overlap other schedule owners; do not add their medians. The native export
completed successfully. Graphics preparation and submission remain substantial,
and all original loading, whole-tick and graphics targets remain open.

Raw comparison logs, binary hashes and receipts remain ignored under
`.cache/codex-evidence/performance-20261023/observation-indexmap-comparison/`.
The native system ranking remains beside the retained baseline capture in
`griffin-celestial-shared-profile/settled-system-ranking.json`.

The regular `cargo build --locked -p lunco-luncosim -j 4` passes in 3m20s.
Its SHA-256 is
`d363f68c2d103e3cd22eeb022c84373fb807465f483bb376a798af1ca94d9dc5`.
The focused USD publication check passes. Production variable projection passes
seven checks on owned API 5223; the missing-state gate passes its expected
terminal fault on 5224. The graphical compiled-metadata gate passes all 35 checks
on 5225 with an inspected success notification and clear runtime diagnostics.
An initial editor launch omitted its fixture scene and never ran the scenario;
it is excluded. Only that gate is rerun with the established replacement scene.

Strict scoped Clippy reports 26 existing lint locations in the worker package,
including redundant clones, collapsible conditions and explicit borrow-ending
drops. No diagnostic identifies the changed publication loop or its added test
cases. These unrelated warnings remain unsuppressed.

The normal windows require exact requested-Twin/physical-root admission,
complete terrain readiness, five seconds of soak and at least ten measured
seconds / 240 fresh coupled ticks. Final images are inspected, registered
diagnostics are clear, source hashes remain unchanged during each run and each
owned session exits:

| Twin | First exact physics readiness | Coupled median/p95 | Frame median/p95 |
| --- | ---: | ---: | ---: |
| Griffin | 13.812 s | 3.505 / 3.998 ms | 10.956 / 13.953 ms |
| Summer | 2.200 s | 3.275 / 3.928 ms | 8.615 / 10.553 ms |
| Tesla | 3.180 s | 1.731 / 2.141 ms | 4.748 / 7.280 ms |

Griffin reaches four-pad handoff; Summer shows its terrain/rover route and Tesla
its stationary car and course. Tesla's existing unavailable frame/datum HUD
remains unqualified. These initial nominal windows are not full-mission gates.
Griffin and Summer have exact source-hash equality with the preceding Celestial
normal cohort; three Tesla authored sources differ despite an unchanged commit.
Concurrent foreign Tesla/video/browser work qualifies timing comparisons. No
whole-scene speed ratio is attributed to this change. The original targets
remain open.

All owned apps and ports 5220–5225 are closed. Normal window receipts and images
are ignored in `{griffin,summer,tesla}-observation-indexmap-normal/`; production
gate records are in `observation-indexmap-production-gates/`. The task-created
baseline test hard link is removed, preserving normal Cargo outputs and shared
caches.

## Native capture qualification

The separate regular Tracy build passes in 2m15s, SHA-256
`6249fc0de41bf1c85fdc1bcb5b5468e089967ba1eec97888cd6871283995ab86`.
The qualified capture uses owned app PID 1760008/API 5228 and collector
1759949/8435, with the same 1 kHz sampling and executor filter as the baseline.
All 271 recorded Twin sources match the baseline and remain unchanged. Nominal
touchdown/handoff, complete terrain readiness, clear diagnostics and the final
image are checked. Both final capture images show the landed view from above.
App and collector exit normally and release their ports.

Native framing contains 16,925 blocks, 388,243,947 compressed payload bytes and
388,311,653 file bytes. Native FixedMain and response-zone exports return zero.
The CPU reader verifies PID 1760008, 1,000,000 ns sampling, 334,600 capture
samples, 1,102 selected main-thread samples, no missing stacks and 240 matching
FixedMain zones. This selection excludes concurrent worker-thread CPU.

The comparison selects 240 complete schedules ending at each cohort's last
measured API tick, excluding fourteen baseline and twelve candidate ticks after
measurement. Native schedule durations are checked against every retained API
observation (seventeen baseline, thirteen candidate). Candidate indices
1715–1954 lie wholly within its measured window (first observed tick 1626).

| Instrumented response handler | Baseline | Candidate |
| --- | ---: | ---: |
| Complete invocations | 478 | 471 |
| Median / p95 | 0.614 / 0.796 ms | 0.657 / 1.196 ms |
| Total | 291.453 ms | 334.291 ms |
| Complete FixedMain median | 4.242 ms | 6.933 ms |

The candidate overlaps a substantial `tutorials` Cargo build and other foreign
work. Its first physics readiness is 28.201 s and API coupled median/p95 is
6.844/10.976 ms with Tracy. This uncontrolled cohort does not establish a
scene-level improvement; the paired consumer remains the isolated timing
evidence. Normal-build measurements above are separate.

The first capture loses a qualification condition despite clear global
readiness and is excluded. Its exact disqualifying fact was not retained; no
root cause is claimed. The qualified driver records individual body, terrain
and handoff facts. An intermediate harness syntax error occurs before app
launch; its owned unconnected collector is stopped and its scratch removed.
All raw data remains ignored in `griffin-observation-indexmap-*profile/` and
`scripts/perf/captures/`.

The final normal build is restored with the canonical locked build command,
passing in 3m22s, SHA-256
`a0699ae1f72ef546e1b4938a3e2644373051b62fcb862487d2a82cc5667fd1ef`.
Application commit `1ff56b52c` is integrated into local `main`, not pushed.
All original acceptance targets remain open. The next startup proposal remains
native Rumoca schedule-proof sharing, pending explicit dependency approval.
