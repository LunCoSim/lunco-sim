# Shared collider hierarchy preparation

`lunco-usd-avian-core` owns rootless collider preparation before Avian scale
cooking and mass reduction. The native propagation entry requires a `Transform`
on a hierarchy root; BigSpace's valid root omits it. The existing bridge remains
the integration owner. No dependency source, API or pin changes.

Preparation shares each validated hierarchy prefix within one invocation.
Each prefix carries the composed native `ColliderTransform`, complete root
depth and ancestor dirty state. An unshared path is checked for cycles and its
full depth, including the cached part, before composition. Rotation validation
precedes publication of a new prefix. Immutable hierarchy/transform queries
make sharing independent of collider visit order. Entries are cleared between
invocations and have no scene lifetime beyond that pass. Plain leaves without
a collider transform are excluded from ancestor invalidation.

The existing top-down arithmetic order, body translation/rotation resets,
inherited scale, omitted-root identity, independent removal streams, diagnostic
ownership and compare-gated output writes are retained. This is continuous
geometry preparation at its Rust owner; no changeable policy, hook, persistent
index or alternative execution path is added. The per-collider full-path
composition is removed.

The source-matched baseline native Summer capture has 240 rootless preparation
zones, 0.622/0.933 ms median/p95 and 158.812 ms total. The earlier exact-zone
CPU selection resolves 148 samples with zero missing stacks: 38 native point
transforms, 30 parent lookups, 18 hash-table operations and 51 system-body
samples. These profiles identify repeated shared ancestry as avoidable work.

The generic ECS diagnostic runs 512 colliders beneath six shared ancestors,
including a body that resets the collider frame, for 256 persistent-system
iterations. Five alternating pairs use independent baseline/candidate copies,
with distinct inodes and hashes verified unchanged after measurement. Every
case retains identical transform fingerprints and exact expected positions,
rotations and scale.

| Changed input | Baseline median | Candidate median | Reduction |
|---|---:|---:|---:|
| Ancestor rotation | 61.134 ms | 12.428 ms | 79.7% |
| Ancestor scale | 64.942 ms | 12.680 ms | 80.5% |
| Ancestor translation | 56.277 ms | 12.261 ms | 78.2% |
| Physical parent | 57.452 ms | 14.868 ms | 74.1% |
| Nonphysical leaf | 20.275 ms | 1.148 ms | 94.3% |

All 17 owner checks pass. The additional low-level check accepts a complete
32-node chain and rejects a child beyond that depth even when its complete
ancestor prefix is already cached. Existing parent-removal, scale/mass,
invalid-rotation and over-depth checks pass. These bounded results establish
preparation savings, not whole-runtime acceptance.

The regular production build passes in 2m15s, SHA-256
`09da73e5ec99ecff70c83b4603045cc5ec09dc616122244f9b542f1d281ad926`.
Normal Summer A/B/A requires five seconds of full physics/terrain readiness,
then ten measured seconds and at least 240 fresh coupled ticks. All 72 admitted
source hashes match, sources remain unchanged, registered diagnostics are
clear and inspected images retain terrain, rover/route markers and shadows.
All three apps close. The independent normal baseline snapshot retains SHA-256
`5cf69a57f1a389a28a0496d3bdb5f8e1ff972440e78c887af73502a7fead85d8`
before and after both baseline runs.

| Normal Summer run | Coupled median/p95 | Frame median/p95 | First physics ready |
|---|---:|---:|---:|
| Baseline, API 5277 | 5.033/6.258 ms | 16.083/21.257 ms | 3.877 s |
| Candidate, API 5278 | 4.738/5.522 ms | 15.546/18.640 ms | 3.039 s |
| Baseline repeat, API 5279 | 5.652/6.791 ms | 16.363/24.606 ms | 3.230 s |

The concurrent Tesla session on API 4499 and baseline variation qualify these
observations. They do not prove clean runtime acceptance or a loading gain.
All original loading, coupled-tick and graphics targets remain open.

The production `invalid_physics_rotation` gate on API 5280 applies its authored
stimulus and passes the expected terminal `avian-backend-pose-invalid` verdict
at tick one. It exits zero without a process panic and releases its port.

Normal Griffin/API 5281 reaches the authored four-pad handoff and sleep, with
clear diagnostics and unchanged source revision
`78d308e4e4fa97ce3d0b07d3b65437f3c95a4de5`. First physics readiness is 35.450 s;
landing/terrain eligibility is 82.596 s. After the soak, coupled service is
9.686/15.389 ms median/p95 and frames 34.434/43.298 ms. The inspected image
retains the lander, terrain, shadow and four-pad handoff HUD. The entire final
`QueryPhysicsState` response matches the previous source-matched normal run,
including velocities, mass, inertia, collider facts and sleep state. That query
does not contain position or orientation; this is not a trajectory replay
claim. The app closes. These slower host-qualified observations do not show a
Griffin runtime gain.

Normal Tesla/API 5282 retains source revision
`a8e7b105f86a9fdf65cbf287501943aefd14fefc`, clear registered diagnostics and a
fully ready stationary measured window. First physics readiness is 9.584 s;
coupled service is 3.363/4.505 ms and frames 18.268/24.284 ms median/p95. The
inspected image retains the car, course, lights and shadows; its existing
frame/datum unavailable HUD remains visible. This window proves readiness and
render continuity, not drive-course acceptance. Sources stay unchanged and
the app closes. No isolated whole-Twin speedup is claimed.

The candidate native capture uses app PID 2637735/API 5283 and collector
2637656/8440, with live listener and connection ownership verified. All 72
source hashes match the normal cohort and baseline capture. Native framing
verifies 10,785 blocks, 236,694,204 payload bytes and 236,737,350 file bytes;
native export succeeds. Fifteen API counter/duration checks select 240 complete
FixedMain zones and exclude 20 screenshot/shutdown ticks. The inspected image
retains the scene and both processes close.

Rootless preparation is 0.257/0.506 ms median/p95, with 69.088 ms total, versus
baseline 0.622/0.933 ms and 158.812 ms. The median reduction is 58.8% in this
source-matched, host-qualified capture. Exact-zone CPU sampling verifies
1,000,000 ns, 240 zones, 63 samples and zero missing stacks. Parent-lookup leaf
samples are six, compared with 30 in the prior diagnostic. Remaining samples
include 35 in the system body and ten native hash-table operations.

Complete instrumented FixedMain is 6.049 ms, versus 6.920 ms baseline. Native
physics schedule median is 1.399 ms, and physical solar visibility is 0.658 ms;
schedule parents and children must not be summed as independent costs. A
separate exact-FixedMain selection verifies 240 zones, 646 main-thread CPU
samples and zero missing stacks. Of those, 606 leaves resolve into the saved
executable and 40 are external. This is a diagnostic lead for the remaining
coupled work, not evidence that the sub-millisecond target is achieved.

The final regular build passes in 3.03 s and restores the exact tested normal
executable hash above. Documentation and formatting do not invalidate the
passing behavior evidence. Temporary independent executable snapshots are
removed after their final identity check; raw profiling data stays ignored.

Raw measurement and build artifacts stay ignored under
`.cache/codex-evidence/performance-20261023/collider-prefix-*`,
`summer-collider-prefix-*` and `scripts/perf/captures/`.
