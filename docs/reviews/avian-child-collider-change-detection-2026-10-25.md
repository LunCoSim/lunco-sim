# Preserve native child-collider pose change detection

Native commit `95d02b32d30a8f3636d70b260a39f05758efbc29` is published on
`LunCoSim/avian` branch `fix/stationary-child-collider-pose` and pinned in
the application. Production replay and bounded landing checks pass; an
end-to-end timing gain remains unproven.

## Owner and observed problem

The preceding Avian pin `9097ce92e5d236c747a68567368575014c28fff5`
unconditionally assigns `Position` and `Rotation` in
`collision/collider/collider_transform/plugin.rs::update_child_collider_position`.
This marks stationary child colliders changed every physics tick.
`collider_tree/update.rs::update_moved_collider_aabbs` checks those change ticks
against `LastPhysicsTick` before computing bounds. The unconditional writes
therefore defeat its existing stationary-collider fast path. The adjacent fork
head `a6681b6` still has these assignments.

Source establishes redundant work. The diagnostic owner timings below do not
establish the isolated saving. Avian owns the repair and the application keeps
its native system.

## Native change and qualification

The native system retains its pose formulas and normalization, and publishes
a component change only when its computed bits differ. Signed zero and NaN
representations are compared explicitly. Every input is still evaluated, so
an independently edited child is restored from its native body and mount even
when those inputs are unchanged. There is no tolerance, new cache, precision
conversion, solver setting or invalid-state suppression.

The patch starts at the application's exact preceding pin in a separate native
worktree. The adjacent `../avian` head and its unrelated joint test edits remain
intact. Three native change-tick tests cover initialization, all body types,
body/mount/owner changes, independent child edits, signed zeros and non-finite
values. The checks pass in 3D/f64 and 3D/f32. The collider-related selections
pass 15 tests in 3D/f64 and 20 tests each in 2D/f64 and 2D/f32. Two existing BVH
fixtures use Parry's dimension-generic ball constructor, replacing their
3D-only constructor so the 2D test targets can compile. This is test-only.

Builds use regular sccache, four jobs and the application's existing `target/`.
The two touched native Rust files are formatted once after validation and the
complete diff is reviewed. All 1,576 non-Avian application lockfile package
records retain their preceding resolutions exactly.

Hook review: exact component invalidation is a physics mechanism invariant
consumed by the native collider tree, with no changeable authored decision.
No application policy or new Rhai hook is introduced.

## Regular production checks

The isolated regular build passes in 3m28s, SHA-256
`786d8504c68775bd8c7edad3243d6770e398a6d18137626bd21488276788ab1f`.
All ten maintained exact-state replay profiles pass 360 authored checks,
including the altered-row discriminator. The reference SHA-256 remains
`6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
Profiles cover four, eight and twenty rovers, single/default Compute pools,
and four jitter/seed combinations.

The first-tick spatial-query positive and negative gates pass 16 and eight
checks; collision enablement passes eight. Invalid rotation still terminates
with the expected `avian-backend-pose-invalid` verdict. All owned gate
processes close and their API ports are released.

The normal Summer comparison retains all 72 source hashes, 1,312 bodies,
1,318 colliders and 16 native contact-graph pairs. Each accepted window has
five seconds of complete readiness followed by ten measured seconds with at
least 240 fresh coupled ticks. Registered diagnostics are clear and the final
images retain the terrain, rocks, route and scene presentation.

| Regular variant | Coupled median/p95 | Frame median/p95 |
|---|---:|---:|
| Preceding pin, API 5341 | 2.443/3.309 ms | 5.961/9.471 ms |
| Native change, API 5336 | 4.030/5.319 ms | 12.729/17.826 ms |
| Preceding pin repeat, API 5342 | 3.333/5.348 ms | 7.031/12.534 ms |

Another worktree's Tesla and a video workload are active. Even the unchanged
baseline median drifts substantially; these observations establish no isolated
speedup or regression. The patch removes a redundant-work path, but its
product timing benefit remains unproven. The original timing targets remain
open. An earlier API 5335 cohort loses full qualification after its first
timing sample and is excluded. That harness did not retain the disqualifying
body/terrain facts; the later passing run does not establish a repair of that
transient.

Griffin/API 5343 reaches four-pad handoff and sleeps without an object pause.
Its 271 source hashes remain unchanged, mass/inertia stay finite and registered
diagnostics contain no errors. First requested-scene physics readiness is
57.224 s and landed eligibility is 91.038 s. The prepared initializer misses
its cache and lowering takes 10.165 s; construction takes 16.777 s with
7,896 native hits and 8,586 compilations out of 16,482 attempts. These cache
conditions differ from the preceding warm run, and native construction precedes
the changed collider system. No loading gain or regression is attributed to
this patch. Coupled median/p95 is 4.583/7.095 ms and frames 19.265/48.523 ms
under concurrent workloads. The inspected image retains the lander, terrain
and shadows. This is a landed window, not complete deployment/egress acceptance.

## Native attribution and regular restoration

The separate Tracy build passes in 237.51 s. Owned application PID 3334358/API
5339 connects to collector PID 3334312 on Tracy port 8444. Both close normally.
The 0.14.1 capture is 261,656,468 bytes: 11,977 blocks and 261,608,554 compressed
payload bytes. Native framing and exports pass; all 72 source hashes match the
normal candidate and the final image is inspected.

The selected 240 completed `FixedMain` instances exclude thirteen screenshot/
shutdown ticks and pass seventeen duration/counter checks. Exact-thread
`FixedPostUpdate` sampling verifies the owned PID, 1,000,000 ns sampling,
584 samples, 240 matching zones and zero missing stacks. The complete schedule
subtree's self durations sum exactly to its 578,457,958 ns inclusive duration.
Child-collider propagation median is 0.002465 ms and native moved-bound update
median is 0.036719 ms. These diagnostic owner costs do not establish a causal
product speedup under the changed host load.

The regular build is restored in 2.79 s. Its hash exactly matches the already
validated normal artifact above, so its passing replay and boundary-gate
evidence is reused. Profiling data remains ignored. Independent executable
comparison copies and the clean native source worktree are removed after final
hash/ownership checks; the published dependency branch remains available.

## Application experiment and decision

Grouping fixed scatter colliders under one separate static body retains all
1,318 Summer colliders and reduces bodies from 1,312 to 715. It preserves the
rocks' native global material defaults, independently of the terrain material.
Its normal coupled median/p95 is 2.434/3.377 ms versus baseline repeats
2.309/2.755 and 2.330/2.653 ms. That representation has no demonstrated gain
and is removed from the application. Independent static rock bodies remain
the production representation. A lower body count alone is insufficient
acceptance evidence.

Raw receipts remain ignored under
`.cache/codex-evidence/performance-20261023/` with prefixes
`avian-child-pose-`, `summer-child-collider-`, `griffin-child-collider-` and
`summer-static-rock-`. The native capture is
`scripts/perf/captures/summer-child-collider-native-profile.tracy`.
