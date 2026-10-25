# Execute the physical commit schedule on its invoking thread

The shared `LunCoSimSimulationPlugin` selects Bevy's native
`SingleThreadedExecutor` for `FixedPostUpdate`. That schedule wraps physical pose
admission, geometry preparation, Avian's solver invocation and writeback.
The existing inner native physics and geometry schedules retain their owners.
Native parallel queries and Modelica worker pools retain their ordinary execution.
No solver algorithm, fixed clock, eight-substep resolution or numerical check
changes. No dependency source is modified.

The preceding qualified Summer capture has a 1.824 ms median for this outer
schedule, including the 0.792 ms native physics schedule and the separate
geometry/pose passes. Inclusive times overlap and cannot be added as independent
costs. A multi-threaded schedule's main-thread self time includes worker waits;
the normal comparison below owns the bounded product gain.

Hook review: this is application execution of an invariant physical commit
chain shared by graphical and headless hosts. It adds no authored decision or
scenario policy, so no new Rhai hook is required.

## Exact replay and normal comparison

The regular production build uses `target/`, sccache and four jobs; it passes
in 55.26 s. Its SHA-256 is
`fd231670fd46b74033bdfc958c8e770aacea33135ddaa1a1e46c038ddea6cf0c`.
All ten maintained profiles replay exactly against the unchanged reference:
360 authored checks, including the deliberately altered-row negative. Profiles
cover four, eight and twenty rovers, single/default Compute pools and four
jitter/seed combinations. The production invalid-rotation fixture also passes
its expected terminal `avian-backend-pose-invalid` verdict without crashing.

The independent baseline normal copy retains SHA-256
`fbd78233deebe3dcfa8905319aad448450034054074a3235c7fd2e30696b08f0`,
with a different inode from Cargo's executable. Its hash remains unchanged
after the A/B/A comparison. All three windows retain the same 72 source hashes,
five seconds of complete readiness and ten measured seconds with at least
240 fresh coupled ticks. Registered diagnostics are clear and apps close.

| Summer variant | Coupled median/p95 | Frame median/p95 |
|---|---:|---:|
| Baseline, API 5322 | 2.349/2.751 ms | 4.858/7.790 ms |
| Candidate, API 5323 | 2.287/2.656 ms | 4.796/7.791 ms |
| Baseline repeat, API 5324 | 2.412/2.768 ms | 4.742/7.811 ms |

The candidate coupled median is 3.94% below the mean baseline medians. This is
one bounded normal A/B/A cohort, not a general gain for every authored model.

Griffin/API 5326 reaches sleeping four-pad handoff with finite mass/inertia,
clear diagnostics and 271 unchanged source hashes. Its first requested-scene
physics readiness is 16.195 s; coupled median/p95 is 3.280/3.876 ms and frames
8.922/12.006 ms. Startup cache conditions differ from earlier cohorts, so the
lower loading time is not attributed to this executor setting.

Tesla/API 5328 retains 128 unchanged source hashes and clear diagnostics.
It is moving during this early driving window: final speed is about
13.8 m/s, finite mass/inertia and no object pause. First requested-scene physics
readiness is 3.318 s; coupled median/p95 is 1.227/1.698 ms and frames
5.593/7.088 ms. The existing frame/datum-unavailable HUD remains. This window
does not certify the complete driving course or its tricks. Images retain
the authored terrain, vehicles and shadows. Both apps close through their API.

The split-table certificate experiment is removed after its independent paired
diagnostic shows a small, mixed benefit. Its
[audit](terrain-occlusion-lookup-2026-10-25.md) records exact work counts;
no cache-layout speedup is claimed here.

## Native attribution

The separate Tracy 0.14.1 build passes in 92.54 s. Owned app PID 3014168/API
5329 connects to collector PID 3014110 on Tracy port 8443; the live socket
receipt verifies both PIDs. App and collector exit normally. The capture is
247,659,798 bytes: 11,296 blocks and 247,614,608 compressed payload bytes.
Native framing and CSV exports pass, all 72 source hashes match the normal
candidate, readiness remains complete, diagnostics are clear and the inspected
image retains terrain and route presentation.

The selected 240 completed `FixedMain` instances exclude fourteen
screenshot/shutdown ticks and pass sixteen duration/counter checks. Exact-zone
CPU selection verifies the owned PID, 1,000,000 ns sampling, 240 matched
`FixedPostUpdate` zones, 677 samples and zero missing stacks. Main-thread
native self accounting sums exactly to the complete outer schedule duration:
679,667,453 ns candidate and 448,385,337 ns preceding capture.

Outer schedule self median is 0.104 ms candidate versus 0.746 ms previously.
The previous multi-threaded schedule's self time includes worker execution and
waits; the candidate executes those surrounding systems on the invoking thread.
It is not a 0.642 ms dispatch-only saving. Full outer medians are 2.809 and
1.824 ms, while full instrumented `FixedMain` medians are 4.966 and 3.331 ms.
The current capture overlaps video playback and has higher individual system
costs. These diagnostic whole-cycle differences are not an isolated comparison
of product performance. The normal A/B/A cohort supplies that bounded evidence.

The regular production build is restored in 1m29s, SHA-256
`6b51d399647d2a9b619c115b87bc75657e22f277d60ab4987c7e8835daa9bc97`.
Its four-rover serial replay passes all 24 checks against the unchanged
reference. All owned application, collector and reader processes close and
their ports are released. Independent comparison executable copies are removed
after their final hash checks; native captures and raw receipts stay ignored.

Loading, sub-millisecond coupled ticks and the complete graphics target remain
open. Raw data remains ignored under `.cache/codex-evidence/performance-20261023/`
and `scripts/perf/captures/`.
