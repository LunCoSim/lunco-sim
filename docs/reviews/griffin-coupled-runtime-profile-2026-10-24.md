# Griffin coupled runtime profile — 2026-10-24

The requested sub-second loading, sub-millisecond complete coupled tick and
sub-10 ms graphics targets remain open. Fresh native Tracy captures separate
airborne execution from settled, sleeping ground contact. Neither instrumented
run is product-performance acceptance. The regular-build observations remain
in [the three-Twin performance review](terrain-certificate-lookup-2026-10-24.md#regular-build-across-the-three-twins).

Both captures use the regular Cargo production Tracy artifact in `target/debug/deps`,
SHA-256 `41fbfffca09fe494ef036562f7cdecff3e67a7f211ddc19dd09d52f8a1ca9e5f`.
It contains the current terrain certificate implementation; later changes before
these captures are formatting, comments and documentation. Native Tracy 0.14.1
readback verifies owned PID, 1 kHz sampling, compressed framing and successful
CSV export. Authored sources remain stable, retained diagnostics are empty,
and the final images were inspected. Another worktree's Tesla session, PID
8277, remains active, so contention also applies.

Each attribution window contains the last 240 complete FixedMain instances.
These are inclusive owner costs; nested rows must not be added together.

| Owner, median ms | Airborne | Landed and sleeping |
| --- | ---: | ---: |
| Complete coupled tick, API | 10.058 | 7.961 |
| Native Avian schedule | 3.639 | 2.040 |
| Connection propagation | 1.876 | 1.803 |
| Exchange write transaction | 1.621 | 1.526 |
| Modelica response handling, per invocation | 1.138 | 1.099 |
| Solar visibility | 0.161 | 0.158 |

The first window ends at simulation tick 1086, before nominal touchdown at
1339. The second explicitly waits for `landing_handoff >= 0.5` before its
readiness soak and measurement; its 57.789-second admission observation includes
waiting for landing and must not be quoted as loading latency. The final
snapshot has 21 sleeping bodies. Both retain 315 broad-phase graph pairs;
that count does not establish 315 touching or solved contacts. Airborne native
contact-solving spans are nearly empty, and the final sleeping snapshot's
contact bias/relaxation costs are below one microsecond each. These observations
do not justify treating contact relaxation as the main cost of either window.

Across all models in the airborne window, 2879 worker integration spans total
2261.051 ms and their checked observations total 844.695 ms. These sums span
worker threads and are not elapsed main-thread time. The last landed snapshot
reports a 5.588 ms main-propulsion worker step. Selected native CPU samples
identify allocation, map/string operations, checked runtime refresh and the
debug-build torn-sweep reference comparison. Finiteness checks and numerical
equivalence checks remain enabled. The input transaction already batches native
writes; the application needs no duplicate input-transaction mechanism.

The largest live constructor takes 4.760 seconds; the coupled initializer's
prepared-solve disk lookup takes 0.874 seconds. Its exact constructor interval
contains 4026 selected 1 kHz CPU samples. Shared-value symbolic construction,
operation cloning and allocation remain visible despite native artifact hits.
This is a focused lead for the existing shared-value proof owner, not evidence
that compilation or all workers run serially.

One application-side correction removes unconditional diagnostic invalidation
on successful Modelica steps. `lunco-modelica-worker` reads the current document
entry before taking a mutable resource reference to clear real diagnostics.
`lunco-doc-bevy` remains the diagnostic owner, and the status bar retains its
existing Bevy change gate. No new cache, API, hook or policy is introduced.
The existing bridge seam test reproduces the false resource change before the
repair and passes afterward; its successful reset also clears a real error,
marks the resource changed and restores the Ready state. No scene-speed gain
is claimed for this small correction.

Evidence stays ignored under `.cache/codex-evidence/performance-20261023/`:
`griffin-detailed-current/` and `griffin-landed-current/`, with matching native
captures under `scripts/perf/captures/`. Both applications, collectors and
readers exited successfully, releasing owned API ports 5114/5115 and profiler
ports 8414/8415. No dependency source changed for this investigation.
