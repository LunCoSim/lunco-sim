# Physics pose invalidation

`lunco-usd-avian-core` owns the conversion between BigSpace's hierarchy and
Avian's native physics frame. Its fixed-step reader excludes plain leaves from
the moved-ancestor set and stops a body's ancestry selection at the active
frame. A leaf cannot be another body's ancestor, and nodes above the active
frame cannot change that body's local physics input. Representation shadows
are still captured on leaves before any later child insertion. Actual physical
ancestor motion, admission, frame transitions and solver ordering retain their
existing owners and consumers. No policy or new mechanism is introduced.

The source-matched native Summer capture has 480 pose-reader calls in 240
complete FixedMain instances, with a 0.343 ms median and 176.863 ms total.
Exact-zone selection resolves 191 CPU samples at 1 kHz with zero missing
stacks: 61 parent lookups, 56 reader-body samples, 31 set lookups, 20 candidate
selection samples and nine Grid lookups. These instrumented times attribute
work; they are not product acceptance numbers.

The bounded ECS diagnostic runs 512 bodies under six common ancestor nodes
and two Grid frames for 256 persistent-system iterations. Five alternating
baseline/candidate pairs use independent executable copies with distinct
inodes and SHA-256 hashes, verified unchanged after measurement.

| Changed input | Baseline median | Candidate median | Reduction |
|---|---:|---:|---:|
| Plain nonphysical leaf | 17.956 ms | 0.453 ms | 97.5% |
| Separate render branch | 17.722 ms | 15.136 ms | 14.6% |
| Ancestor above the active frame | 20.290 ms | 16.582 ms | 18.3% |
| Physical ancestor below the frame | 123.283 ms | 122.239 ms | 0.8% |

Every diagnostic run checks exact native positions and zero velocity, including
the expected 3.75 m final physical-parent displacement. It also checks the
changed leaf's final representation shadow. Both variants pass all 16 owner
mechanism tests. The diagnostic's results establish bounded selection savings,
not a whole-tick gain. No solver, timestep, precision or graphics settings change.

The regular production build passes in 2m33s, SHA-256
`91d0da205a9a564fefce2e2817892886ca6230ca96bcf7644586ba8fb9ce02e5`.
Summer's normal A/B/A runs require five seconds of full physics/terrain
readiness, then ten measured seconds and at least 240 fresh coupled ticks.
All 72 admitted source hashes match across the three windows. Both baseline
runs use unchanged independent executable SHA-256
`06a14b637793c49a72f679cbef1aefbe02cb43af202a3129c119acdd634cff25`.

| Normal Summer run | Coupled median/p95 | Frame median/p95 | First physics ready |
|---|---:|---:|---:|
| Baseline, API 5270 | 5.233/6.144 ms | 16.215/20.670 ms | 3.485 s |
| Candidate, API 5271 | 4.948/6.080 ms | 15.985/20.082 ms | 2.847 s |
| Baseline repeat, API 5272 | 5.353/6.402 ms | 16.748/22.039 ms | 3.756 s |

Registered diagnostics remain clear, sources stay unchanged, inspected images
retain terrain, rover/route markers and shadows, and all three apps close.
The concurrent Tesla session on API 4499 qualifies these observations; they
support the bounded diagnostic but do not establish clean performance
acceptance or a loading improvement. All original targets remain unmet.

The production `invalid_physics_rotation` gate on API 5273 applies its authored
non-unit ancestor rotation and passes the expected terminal
`avian-backend-pose-invalid` verdict at tick one. It exits zero without a
process panic and releases its port.

Normal Griffin/API 5274 reaches the authored four-pad handoff, with a sleeping
lander and all final velocity components below 0.0001 in their native units.
First physics readiness is 30.837 s; full landing/terrain eligibility is
66.083 s, followed by the same soak and measured window. Coupled service is
7.754/9.738 ms median/p95 and frames 26.910/33.597 ms. Source revision
`78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5` remains unchanged, diagnostics are
clear, the inspected image retains lander/terrain/shadow and the authored
four-pad handoff HUD, and the app closes.

Normal Tesla/API 5275 retains source revision
`a8e7b105f86a9fdf65cbf287501943aefd14fefc`, clear registered diagnostics and a
fully ready measured window. First physics readiness is 7.227 s; coupled
service is 2.890/3.784 ms and frames 13.065/17.189 ms median/p95. The inspected
image retains the car, course, lights and shadows. Its existing frame/datum
unavailable HUD remains visible. This stationary window proves readiness and
render continuity, not drive-course acceptance. Sources stay unchanged and
the app closes. Both Twin runs share the same normal candidate executable;
host contention qualifies their timings.

The candidate's native Tracy capture uses app PID 2522014/API 5276 and
collector 2522006/8439, with live listener/connection ownership verified.
All 72 source hashes match the baseline capture and the normal cohort.
Native framing verifies 9,779 blocks, 218,448,900 payload bytes and 218,488,022
file bytes; native export succeeds. Fourteen API counter/duration checks select
240 complete FixedMain zones and exclude 22 screenshot/shutdown ticks.
Exact pose-zone sampling verifies 1,000,000 ns, 480 zones, 130 CPU samples and
zero missing stacks. The image is inspected and both processes close.

The pose reader's native median/p95 is 0.265/0.461 ms, versus baseline
0.343/0.562 ms; total time is 139.028 versus 176.863 ms over 480 calls.
Parent-lookup leaf samples fall from 61 to 25. Complete instrumented FixedMain
remains 6.920 versus 6.940 ms, while rootless propagation is 0.622 ms. These
source-matched, host-qualified captures support the selection attribution;
they do not establish clean whole-runtime acceptance.

The final regular build passes in 2m24s, SHA-256
`5cf69a57f1a389a28a0496d3bdb5f8e1ff972440e78c887af73502a7fead85d8`.
No behavior changes follow the validated runs; their results are retained.
The temporary independent executable snapshots are removed after their final
identity check. All owned API and Tracy ports are closed. Unrelated handoff
edits remain outside this change.

The next separate lead is rootless collider propagation. Its same-window
240 native zones have a 0.590 ms median. Exact-zone sampling resolves all 148
samples with zero missing stacks: 51 in the system body, 38 native
`ColliderTransform::transform_point`, 30 parent lookups and 18 hash-table
lookups. The retained pose change does not modify that propagation algorithm.

Raw profiles, comparison receipts, build logs and runtime artifacts remain in
ignored `.cache/codex-evidence/performance-20261023/pose-invalidation-*`,
`summer-pose-invalidation-*` and `scripts/perf/captures/` paths. Reusable source
and documentation are source controlled; raw measurements are not.
