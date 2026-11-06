# Fixed exchange, terrain certificates and solver-profile diagnostics

Application base: `c99e77408`. Maintained Avian patch:
`b8d1f5a3d5aeeb61220813e82607ca6a74e7057a`, published and remote-verified on
`LunCoSim/avian:fix/checked-inertia-numerics`.

## Owners and invariants

- `lunco-port-core` owns scoped read/preflight proofs. Registry, World, entity,
  revision and full precedence prefixes remain checked. Owner keys are reused
  only inside one uninterrupted immutable World borrow, then invalidated even
  when the transaction rejects. Resolved staging borrows metadata and retains
  numeric commit data; retained prepared writes keep their later-World checks.
- `lunco-cosim` compiles distinct source endpoints, samples each once at a fresh
  communication point, and accumulates every affine wire in its admitted order.
  Source reads and all-or-none input staging use the same registry transaction.
  Holds, required-source failure, backend precedence and force reapplication
  keep their authoritative paths.
- `lunco-terrain-surface` owns bounded exact certificates. Coarse cells use
  stable 4-by-4 page slots; rectangles retain exact coordinate keys. Empty-page
  retirement invalidates its hot slot before reuse. Surface fencing, FIFO
  capacity, work admission, f64 bounds and classification order are unchanged.
- Avian binds current solver body indices once while its native mapping and
  joint components are borrowed. The same prepared records feed all relaxation
  passes. Static/disabled filtering, entity order, math and factor preparation
  remain native; default eight substeps and sixteen relaxation passes remain.
- `lunco-physics` owns resolution and publishes actual native changes.
  `lunco-scene-validation` supplies typed facts to startup-installed
  `lint.physics`; Rhai owns the advisory threshold, severity and message. The
  shared lint report and status telemetry are its actual consumers. The rule
  reports more than eight as an error-level potential performance issue while
  preserving the supported 1–64 range and running simulation. Equal writes do
  not republish. Restoring eight clears current findings. Scene teardown retires
  caches and wakes the producer; policy generations invalidate the decision.
  Explicit RunLint preserves its caller context and rejects document-scoped
  live-physics requests. No parallel UI/error mechanism or runtime fault is added.

See [lint contracts](../architecture/lint-substrate.md),
[port contracts](../architecture/efficiency-and-maintainability.md), and
[terrain certificates](../architecture/terrain-layered-rendering.md).

## Measured mechanisms

Five alternating normal release trials use independent before/after test ELFs,
identical inputs and exact final-value checks. Timings below are medians; these
are mechanism measurements, not whole-frame or Modelica worker measurements.

| Existing fixture | Before ms | After ms | Reduction |
|---|---:|---:|---:|
| Scoped input transaction, 262144 writes | 22.701 | 12.428 | 45.26% |
| Exchange, one source / 128-way fanout | 256.319 | 136.367 | 46.80% |
| Exchange, 32 sources / four-way fanout | 255.000 | 162.134 | 36.42% |
| Exchange, 128 distinct sources | 262.182 | 245.104 | 6.51% |
| 768000 resolved surface writes | 168.291 | 131.724 | 21.73% |
| Warm terrain traversal, 257 grid / 65536 capacity | 51.641 | 48.974 | 5.17% |

Terrain miss-heavy medians change by -0.28%, -0.72% and -1.61% (slower). The
certificate-store change therefore has a modest warm-cache gain with measured
miss-side overhead. It does not establish an overall solar-visibility gain.
Concurrent-build pilot timings are excluded; quiet receipts retain process
inventories. All admitted ray/interval/certificate counts agree.

Artifacts: `target/perf/fixed-owner-implementation-20261105/`, especially
`quiet-mechanism-receipts.json`, `mechanism-summary.json`, the independently
retained test ELFs, production logs, schemas and screenshots.

## Verification and boundaries

Port mechanisms: 32 pass; propagation: 15 pass; certified occlusion: 11 pass.
The minimal missing-owner resource seam passes its named Rust test.
The production native joint, G7 joint, continuous-contact and owner-collision
fixtures pass 8, 6, 12 and 12 authored checks respectively.

`continuous_articulated_mass` fails the same two checks before and after, with
identical reported positions: foot x=38.29972267150879 instead of 2.4 and root
x=36.29972267150879 instead of 0.4 after three ticks. This is an inherited
articulated CCD defect, not a qualified contact-convergence result. Tolerances
remain intact; Rhai failure messages now retain actual/expected positions.
`cosim_chain` requires the optional Python feature; this normal binary reports
the amplifier unavailable, so that failed run is excluded from qualification.
The dedicated Rhai input-contract fixture is the applicable scalar/force gate.

## Integrated production qualification

`cargo build --release --locked --offline -p lunco-luncosim -p gen-command-docs -j 4`
uses the published Git pin without a path override. The retained normal binary
SHA-256 is `1fa0289a78262b492a23e00e6172edd8b5942d60c392b669fb35f18cc8ca03c1`.
The published native source matches the reviewed local commit exactly. The lock
change contains only Avian source identities and the two new owner dependencies.

The integrated production run passes 15 solver-profile/input-scope Rhai checks,
16 co-simulation input checks, eight daylight solar checks, two night checks and
the authored invalid-solar expected-fault gate. At 16 substeps the retained
`pinned-profile.high.json` shows one error-level lint while readiness remains
`ready=true`, `world_hold=false`, `faulted=false`; the screenshot shows its native
status consumer. A host without native physics fails its diagnostics scope
explicitly. Command documentation is regenerated with all 316 commands retained,
using fresh GUI/headless schemas plus the ten unchanged networking entries from
the retained production schema; their owning source is unchanged.

A separate uninstrumented High/2560×1440 Griffin run records 514 frames at
**47.726 FPS**, 20.734 ms median and 25.546 ms p95. The last native physics window
is 1.204 ms median / 1.603 ms p95. The final 240 coupled fixed ticks are 3.673 ms
median / 5.056 ms p95. Across exposure windows, aggregate Modelica worker service
is 3.531 ms per authoritative physics step (median), with thirteen participants;
the median largest-participant mean is 1.590 ms/job. Worker service is attribution,
not coupled-step wall-clock latency, and must not be added to the latter.
The native health physics cadence is approximately 60 Hz; non-atomic API poll
counter/wall-time deltas are retained separately in `normal-summary.json`.

The scene reaches engine cutoff and confirmed touchdown. This bounded landing
window does not certify the complete delivery/rover mission. All 548 guarded
model source hashes are unchanged through admission and shutdown. Readiness,
terrain/camera qualification and diagnostics pass; runtime diagnostics report
zero errors/warnings. A native telemetry retention-cap warning is present in the
log, independently of those diagnostics. No compiler appears in the qualified
workload snapshot; the ordinary desktop/compositor and editor remain active.
The owned app and port 6014 are confirmed absent/released.

This fresh candidate is not a paired FPS improvement claim: the earlier normal
runs were compilation-contended and precede the latest HUD changes. A new Tracy
capture was not taken for this implementation; the earlier capture supplied
hotspot attribution and the independent fixtures quantify the changes. The
1 ms Modelica and 150 FPS targets remain unmet. LunCoSim edits remain local;
only the explicitly authorized Avian branch was pushed.
