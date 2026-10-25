# Execute native geometry preparation on the invoking thread

`PhysicsGeometryPreparation` uses Bevy 0.20's native `SingleThreadedExecutor`.
The physics owner configures it once when installing the shared preparation
schedule. Its short dependency chain supplies rootless collider transforms,
cooks native scale/mass and performs the existing body-owned reduction. These
passes execute together in the invoking thread.

This is execution of an invariant engine chain, so Rust's physics plugin owns
the setting. No authored selection rule or new Rhai hook is required. The
native algorithms, preparation boundaries, clock, solver resolution and f64
domain contracts retain their existing owners. No dependency source changes.

## Behavioral qualification

The regular production build uses sccache, `target/` and `-j 4`. Its SHA-256 is
`a24fe4379f048e188e001be03cce9250cc7789cd99406881941a241f8f97e4d6`.
All ten maintained profiles replay exactly against the unchanged reference,
with 360 passing checks including the deliberately altered-row negative.
They cover four, eight and twenty rovers, single/default Compute pools, and
four jitter/seed combinations. `invalid_physics_rotation` also passes its
expected terminal `avian-backend-pose-invalid` verdict without crashing.

Normal Summer/API 5309, Griffin/5311 and Tesla/5312 windows retain clear runtime
diagnostics and unchanged admitted source hashes. Griffin reaches sleeping
four-pad handoff. Inspected images retain terrain, geometry and shadows.
Tesla's stationary window retains its existing frame/datum-unavailable HUD;
it is not complete driving-course acceptance. All owned processes close.

## Normal comparison

The baseline artifact is an independent copy of the validated normal binary
`fcd1c9b352bd9b9ecf9f2d8f47f6dc76ab68ea788c5447f1e0aedd595e038a5f`.
Its inode differs from the compiler-managed executable, and its hash is checked
before comparison. Every window follows five seconds of complete readiness,
then ten measured seconds with at least 240 fresh coupled ticks. Summer's 72
source hashes match across the A/B/A cohort.

| Summer variant | Coupled median/p95 | Frame median/p95 |
|---|---:|---:|
| Baseline, API 5307 | 2.422/2.774 ms | 4.783/7.987 ms |
| Candidate, API 5309 | 2.325/2.683 ms | 4.760/7.954 ms |
| Baseline repeat, API 5310 | 2.416/2.835 ms | 4.660/8.067 ms |

The candidate's coupled median is about 3.9% lower than the mean baseline
medians. These are host-qualified observations with another Tesla session on
API 4499 still active. Earlier runs have substantially different host costs;
their larger timing differences are not isolated gains from this change.

The additional normal Griffin window measures 3.341/3.938 ms coupled and
8.947/11.780 ms frames. Tesla measures 1.506/1.757 ms coupled and 5.503/6.356 ms
frames. Neither is an A/B comparison. Griffin's first requested-scene physics
readiness is 21.999 s. All original loading, coupled-tick and graphics targets
remain open.

## Native attribution

Separate source-matched Tracy captures use the adjacent 0.14.1 tools, 1 kHz
samples and the same executor-bookkeeping filter. Baseline PID 2811282/API
5306 and candidate PID 2840666/API 5314 both finish and release their ports.
The candidate's live listener/connection matches that PID and collector
2840612 on port 8442. Native framing, exports and expected-PID sample selection
pass. The candidate capture is 243,098,914 bytes with 11,462 blocks and
243,053,060 compressed payload bytes.

Both windows select 240 completed `FixedMain` instances. The candidate excludes
11 screenshot/shutdown ticks and passes 18 duration/counter checks. Geometry
preparation's median/p95 is 0.454/0.782 ms baseline and 0.221/0.339 ms candidate.
Native self accounting equals the complete geometry duration exactly:
119,009,297 ns baseline and 56,679,626 ns candidate. Main-thread self spans in
the baseline include worker execution and waits; they are not pure executor
overhead. In the candidate, rootless propagation and all geometry passes occur
on the invoking thread; schedule self median is 0.020 ms.

Whole instrumented `FixedMain` medians are 5.809 and 3.331 ms, with different
host pressure and presentation counts. This is diagnostic attribution, not a
claim of that whole-cycle gain. The normal A/B/A cohort bounds the observed
product effect. Candidate native physics, physical solar visibility and Modelica
integration remain measured leads. The separate
[Rumoca workspace proposal](rumoca-torn-sweep-workspace-2026-10-25.md) awaits
explicit dependency authorization.

The canonical regular build is restored, SHA-256
`1cda691ae3b10c167f4d0fd3cba70b8f6736c85d404a6300d024683006111163`.
Its four-rover serial replay also passes all 24 checks against the unchanged
reference. An unsupported version-flag probe launched an extra task-owned
window after the performance windows had ended; that verified process and its
command shell were stopped without launching the queued second probe. It is
excluded from performance evidence.

Raw data remains ignored under `.cache/codex-evidence/performance-20261023/`
(`summer-geometry-executor-*`, `griffin-geometry-executor-normal`,
`tesla-geometry-executor-normal`, `geometry-executor-*`) and
`scripts/perf/captures/`. Owned independent comparison copies are removed after
their final hash checks. All owned application and collector processes close.
