# Coupled certification readiness

The coupled initialization coordinator now waits for participant outcomes before
running full certification. Native solver construction remains asynchronous.

Owner: `lunco-usd-sim-cosim::initialization`. Its existing `needed` ECS run
condition reads the current certificate and its member `ModelicaModel` states.
Pending members close the condition. All completed members, a member error or
a missing model wake the same coordinator. The result only schedules owner
work; source, authored values, physical samples, wiring, solver time and
connected observations still pass the complete certification boundary before
the group is released. Scene teardown retains its existing owner retirement.
This is an engine readiness invariant rather than changeable policy; no new
Rhai hook, command, state resource or dependency is introduced.

The source-matched baseline capture `griffin-loading-native-property` contains
274 coordinator executions totaling 665.214 ms. Of these, 252 executions and
539.640 ms occur in the 16–24 s interval, while participant constructors are
preparing after the one-shot coupled result. Repeating physical sampling and
full certification cannot establish readiness while those observations are
still unavailable.

## Verification

- The focused `certification_waits_for_participant_outcomes` ECS seam passes.
  It covers pending members, partial completion, complete outcomes, an error
  while another member waits, and a missing model.
- The regular production build passes 44 authored initialization/clock checks
  and two receipt/scene-retirement checks on owned PID 1048950/API 5203.
- The authored inconsistent-equations fixture reaches its declared terminal
  `modelica-initialization-failed` fault and exits zero on PID 1049243/API 5205.
- The regular Griffin run uses model revision
  `78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`, with identical authored hashes to
  the preceding native-property run. PID 1049545/API 5206 reaches observed
  requested-Twin/native-physics readiness at 17.517 s and landed eligibility
  at 39.476 s. Four-pad touchdown and flight handoff occur, diagnostics are
  clear, and the inspected image shows the lander, terrain and shadow.

The normal binary SHA-256 is
`200b289ee9292921ecd97cd5cf988752c2ed3cc588b1d6a6d6003e8b2b9fdf1d`.
Complete coupled-tick median/p95 is 4.052/4.870 ms; graphics is
11.876/15.645 ms. No end-to-end loading gain is established: the preceding
normal observation is 13.302 s by the same exact-root readback. Unchanged
native initializer work also grows from 2.891 to 3.556 s before this gate
can change execution. Its source key and native attempt/hit/compile counts
remain equal. Concurrent application/browser workloads and recorded pressure
qualify timing comparisons. All loading, physics and graphics targets remain
open. The source-matched final startup capture observes 19 coordinator executions
totaling 56.500 ms, versus 274 and 665.214 ms before the change. After consuming
the one-shot result at 17.345 s, the coordinator next executes at 21.366 s
to certify the completed participants. Main updates continue between these
boundaries, with no repeated coordinator execution. This establishes removal
of pending-constructor work, not the requested end-to-end loading gain.

The capture is 212,135,956 bytes with 9,606 native blocks and 212,097,526
compressed payload bytes. The collector and native aggregate/exact-zone
exporters exit zero. The native sample reader verifies PID 1090355,
1,000,000 ns sampling, 19 matching zones and no missing stacks. Owned
API 5207 and collector 8429 close normally; authored hashes remain unchanged,
diagnostics are clear and the startup image is inspected. Concurrent video
encoding is recorded, so this startup-only capture supplies no settled FPS
acceptance. The readiness observation is 17.883 s with Tracy.

All production sessions above exit and release their ports. Raw captures,
process ownership, hashes, pressure records, logs and images remain ignored
under `.cache/codex-evidence/performance-20261023/` and
`scripts/perf/captures/`.
