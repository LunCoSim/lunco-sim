# Factory door interaction and composed metadata queries

The scene-query owner supports `position: false` on both `QueryUsdPrim` and
`QueryUsdPrims`. Placement remains the documented default. A metadata-only
request skips authored world-transform evaluation and live physics pose reads.
The canonical owner admits an exact plan identity with a projection generation
and a composition/resolver read lease. Native edits invalidate that read lease
before composed projection notices drain; a fresh, matching worker plan can
establish it at a later generation. Native child lists, geometry, topology and
placement retain their native query contract. A source-entity-scoped query and
`find_path_in_stage` share the exact mount and unique live projection lookup in
`UsdPrimIndex`. No additional startup preloading or stage traversal is installed.

The Factory walkthrough's authored relationships identify all controlled doors,
their leaves, overhead collision targets and ordered lift destinations. Rhai
reads one control record and batches the leaf facts; all live poses are read in
the active frame with `world_pose`. It constructs a complete pose, collider and
view transition before invoking `SetSceneParticipation`. The native owner
validates all destinations before changing the world and shares its pose
preparation/application with `TransformEntity`. Lift or door states that cannot
be resolved to an authored endpoint reject without mutating other targets.
There is no second live door state cache.

## Debug evidence from the initial selective-query optimization

Production debug executable built in this checkout (before the final native
pre-commit invalidation refinement), Factory source baseline
`3efedb4a0dacdabbc905f7a6bfa495fca9c7b0ad`, edited walkthrough compared to the
original with the same executable. A private Xephyr Vulkan window (1600×1000)
was used to avoid altering the user's desktop session. Four owned sessions
measured callback wall time using Rhai's monotonic timer, six hinged and six
overhead interactions per session, plus four non-door clicks. Medians pool the
repeated clicks (excluding the first per category per session).

| Callback | Original median (ms) | Optimized median (ms) |
| --- | ---: | ---: |
| Hinged door | 0.854 | 0.674 |
| Overhead door | 1.121 | 0.769 |
| Non-door click | 0.290 | 0.043 |

First hinged clicks were 1.021/10.788 ms in the original runs and 6.039/0.710 ms
in the optimized runs. First overhead clicks were 1.212/7.267 ms and
1.465/3.897 ms respectively. Cold interactions remain variable; these samples
do not qualify an every-frame or every-first-click budget. In a separate
alternating in-process comparison on the optimized scene, three groups of 100
explicit document leaf queries with placement took 9.129, 8.696, 7.602 ms;
without placement they took 1.714, 1.588, 1.814 ms. Medians are 8.696 and 1.714
ms (5.07×). Result checks require the requested attributes and the exact
presence/absence of placement.

A same-path startup bracket alternated the original and optimized walkthrough in
one private Twin root with the same binary, using an atomic readiness/Twin/root
observation. Original startup-to-readiness: 5.094 and 4.037 s; optimized:
4.612 and 3.692 s. Separate-path initial comparisons varied before scenario
admission and are not a startup regression measurement. This bracket does not
claim an absolute load-time target.

The initial optimization's debug build passed the canonical-stage unit suite (61 passed, one
ignored), including the exact recipe lease and direct layer edit before notice
drain. The production `usd_query_api` scene passed eight authored checks,
including position omission, single/batch equivalence, invalid option and
missing-path rejection, preserved f64 placement and current authored values
after edits. `collision_enabled` passed 18 checks and `native_xform_animation`
passed ten checks.

On that debug binary Factory `live_door_controls.rhai` passed nine checks
(hinged/double/overhead poses, overhead collider/visibility, rejected stale
identity, non-door click, unchanged document generation and no projection).
The Site Layout HUD gate passed overlays, live infeed motion, all three lift
stops and ten cameras. Six injected native window clicks passed on the initial
debug build with screenshots inspected; on that build three succeeded
before the fourth test injection did not toggle. The checkout's generated
`target/` directory was removed externally before that run's event log could
be inspected. A separate repeat using the available main-worktree debug
binary (`11b4d2ed1`) and the exact updated Factory script verified all eight
native injections reached the same door (one pointer event per click) and
changed its live pose. It is a pointer-policy qualification, not a final-engine
performance measurement. All owned sessions exited.

The debug nested-display frame history is diagnostic, not native desktop FPS
acceptance. No release executable or changed graphics quality was used for
qualification. The original timing receipts and logs were removed with the
build directory; the independent pointer repeat is retained under ignored
`target/perf/factory-door/final-pointer/`.

## Architecture follow-up: production Debug verification

The actual Factory Twin is `/home/rod/Documents/models/factory`, not an archive.
The source was edited through `OpenFile`, `InspectUsdDocument`,
`ApplyUsdOps`, and `SaveDocument`, with relationship and offset readback.
The scene host `/World` owns the control topology; the authored program
declaration under `WalkthroughControls` is not the runtime host. All 43 doors
declare exact leaf relationships; the three overhead leaves declare exact
collider relationships; three lift landing offsets are explicitly authored.
The existing plan-derived generator produces the same topology.

A native OpenUSD Sdf comparison against the byte-identical original source
found 20,007 original specs, 20,118 resulting specs, 155 additions (52 authored
properties plus relationship target specs), 44 removals (retired authored live
state), and **zero changed shared field values**. The native writer reformatted
the text from 6.17 MB to 11.94 MB; there is no geometry, transform, material,
physics, or reference change. The independent Factory saved-geometry gate
passed all 43 openings, lift/shaft clearances, stairs, envelope, glazing,
equipment and 12 pipe penetrations. Its standalone OpenUSD resolver cannot
resolve the engine's `lunco://` camera reference; the production GUI verifies
that camera and the other nine camera choices.

The new Debug production executable passed:

- `collision_enabled.usda`: 19 checks, including rejection of a later invalid
  quaternion without changing an earlier pose, body, collider, or view.
- `usd_query_api.usda`: nine checks, including exact source-entity scope,
  retired/conflicting source rejection, and current metadata after edits.
- `selective_composed_queries_reuse_only_the_exact_composition_lease`: one
  focused mechanism test, including a direct layer edit before projection
  notification and a fresh replacement snapshot at a nonzero generation.
- The actual Factory live-door observer: 12 checks, including hinged,
  double, and overhead doors, stale and non-door events, two lift calls,
  rejection of a deliberately mixed landing without changing any other
  target, and unchanged document generation/no reprojection.
- Eight actual window pointer events: each produced exactly one hit on the
  expected door panel and changed its live pose. Before/after screenshots
  were inspected. Every Site Layout HUD control passed, including all three
  lift stops, moving/idle infeed treads, overlay meshes, and ten cameras;
  authored generation remained unchanged.

A four-run startup bracket alternated the previous optimized and current
source bytes at the same private Twin path (same Debug binary, same manifest
and display). Previous readiness times were 4.816 and 12.248 s; current times
were 4.923 and 9.074 s. The medians were 8.532 and 6.998 s respectively.
These are variable host measurements, not a guaranteed startup budget.
There are no new preloads or cache-building startup tasks.

Separate nested-display frame-history samples used the actual Factory path
and the prior optimized reference copy. Current quiet mean was 53.996 ms;
median of eight click-window means was 52.250 ms, with a maximum observed
sample of 107.380 ms. Previous quiet mean was 48.704 ms; median click-window
mean was 45.021 ms, maximum 95.115 ms. Other simulator/control workloads were
running; these are diagnostic frame windows, not a clean FPS benchmark or
evidence that every click is free of spikes. Both runs had zero runtime
diagnostic findings and eight correct native pointer events. No Release
build was used. Evidence is retained in
`target/perf/factory-door/architecture/` (ignored runtime artifacts).

On a fresh production Debug load of the saved actual Twin, six alternating
100-query blocks of the exact source-scoped leaf read gave full-pose samples
of 2.674, 3.023 and 2.474 ms and selective samples of 1.457, 1.432 and
1.446 ms. Median 2.674 versus 1.446 ms (1.85×); every record preserved the
same authored attributes and the requested presence/absence of the live
pose fields. The final cleaned Factory script repeated its 12-check authored
verdict successfully in that same session.
