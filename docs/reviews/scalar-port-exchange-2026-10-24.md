# Scalar-port exchange — 2026-10-24

The port owner retains live precedence, metadata, value, locator and duplicate
validation before an atomic commit. Private port-name indexes and duplicate sets
use Bevy's `FixedHasher`; public enumeration and commit order still follow the
existing slot/staging sequences, with full key equality. Public map conversion
APIs and topology fingerprints retain their contracts. Scalar endpoint metadata
copies directly from the admitted contract, avoiding an intermediate unit/frame
clone. Regular development builds optimize the two hot-loop packages,
`lunco-port-core` and `lunco-cosim`, at level 3 with assertions enabled.

This changes application-owned lookup/code generation, without a new cache,
public API, schema or policy decision. `PortMap`, `InputWriteBuffer` and the
registered scalar backends remain the owners; co-simulation exchange is the real
consumer. No dependency source changes are needed.

The retained manual benchmark measures 6000 atomic batches of 128 resolved
surface inputs. Every input has a unit, frame and bounds, and the final values
must retain their exact f64 bits. With identical benchmark inputs, 768,000 writes
take 291.782 ms before and 195.468 ms after, a 33.0% reduction. This measures the
combined implementation/profile change, not each part independently or a whole
physics tick. All 27 port-owner checks and 66 co-simulation checks pass; the two
manual co-simulation benchmarks remain ignored in the ordinary suite. The
regular production ownership scene passes all twelve checks, including its
negative cases. The skill catalogue passes all 43 checks.

The regular production rebuild completes in 4m00s, with the existing unrelated
`UsdStageProjectionPlan` unused-import warning. Its SHA-256 is
`9c3f5d62c42034cbf9ceac64dc4c476b5a59b0ebdeacbba055d5cef1b1e0cbef`.
Fresh graphical runs use the same admitted authored source hashes as
[the preceding regular-build cohort](terrain-certificate-lookup-2026-10-24.md#regular-build-across-the-three-twins).
Each requires the intended native body, fully resident terrain, completed derived
maps, five seconds of sustained readiness and a ten-second measurement window.
Each coupled-tick summary contains 240 complete samples.

| Twin | Fully-ready time | Coupled tick p50 / p95 | Frame p50 / p95 |
| --- | ---: | ---: | ---: |
| Summer Space School | 4.398 s | 5.176 / 6.211 ms | 13.069 / 16.586 ms |
| Griffin / lunar-base-model | 35.060 s | 7.910 / 9.909 ms | 22.569 / 32.484 ms |
| Tesla Roadster | 3.054 s | 2.057 / 3.061 ms | 6.699 / 10.101 ms |

All runs retain zero diagnostics and script errors and stable authored sources.
Their final images were inspected. Griffin reaches its nominal touchdown
handoff; these bounded observations do not constitute complete mission verdicts.
The Tesla HUD still reports unavailable frame/datum facts, so this run does not
qualify that presentation contract. Another worktree's Tesla session, PID 8277,
remains active. These are contention-affected observations rather than clean
acceptance or a causal whole-scene speedup. Every requested performance target
remains open.

A separate regular Tracy build completes in 2m59s with the same existing warning.
Its SHA-256 is
`4a896c7bdd366cee039a573e4a85674ee45ea1feb543c5371dab178ff82ea5f2`.
The owned Griffin capture waits for the landing handoff, sustained readiness and
a ten-second measurement. Native Tracy 0.14.1 readback verifies PID 619583,
1 kHz sampling, all 16,644 compressed blocks, the collector's 333.29 MiB payload
and successful native export. Its authored source hashes match the preceding
landed capture, and its final snapshot retains 21 sleeping bodies, 20 dynamic
bodies, 777 colliders, 31 joints and no orphan graph pairs. Its final image and
diagnostics are checked.

The last 240 complete FixedMain instances cover 77,322,464,936 through
83,000,769,406 ns of the capture. Inclusive owner costs are:

| Instrumented owner | Before | After |
| --- | ---: | ---: |
| Exchange write median | 1.526 ms | 1.318 ms |
| Exchange write total | 380.767 ms | 332.939 ms |
| Complete propagation median | 1.803 ms | 1.589 ms |
| Complete coupled tick median, API | 7.961 ms | 7.647 ms |

Exchange write median is 13.6% lower and total is 12.6% lower in this diagnostic
comparison. Parent and child costs must not be added. Profiler/workload effects
and different native trajectories remain limits on causal whole-scene claims.
Modelica response handling is still 1.194 ms per invocation; native Avian is
1.872 ms per schedule. Rootless collider-transform propagation and repeated
pose admission remain attribution leads at the application physics adapter;
their algorithms have not changed in this batch. Loading is also still
dominated by preparation/construction rather than this lookup improvement.

Raw logs, source hashes, build receipts and benchmark measurements stay ignored
under `.cache/codex-evidence/performance-20261023/`, using `port-exchange-*` and
`{summer,griffin,tesla}-port-exchange-normal/`. The pre-format implementation diff
and its hash are retained there because the production builds precede the final
format/review/commit boundary. No profiling data is committed. All four numeric
gate/regular-run applications exit successfully, releasing owned API ports
5116–5119.
The additional capture and native exports use `griffin-port-exchange-landed/`
and the matching ignored `.tracy` file. Its application, collector and native
reader exit successfully; owned API port 5120 and profiler port 8416 are released.
