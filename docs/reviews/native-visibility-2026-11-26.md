# Retained exact native camera and directional visibility

The maintained Bevy revision `87cb795e3f11da0d769d28e57df0ec42a7112aa8`
owns both native CPU visibility passes. The camera owner retains sorted
class-specific lists and an exact visibility union; directional lights retain
sorted per-view cascade lists and a shared immutable union. Equal computed
view geometry and layer/culling facts permit sparse reconciliation of changed
candidate decisions. Optional and required component removals, marker changes,
entity retirement and final-state re-admission are included. Visibility ranges
are always read from their current owner. For broadly dirty inventories or a
new/changed view, the existing parallel full culling predicates remain the
authority; no approximate geometric test, reduced cadence or quality setting
is introduced. Retained lists are re-applied to `ViewVisibility` every frame.

The focused native integration tests prove that unchanged views do not rewrite
their output while still re-marking visible entities. They cover layer and
marker transitions, duplicate class membership, entity retirement, light
disable/re-enable and cascade-count change. All 15 related owner tests passed,
including the existing conservative receiver-hull cases. The manifest pins
only the two new owners, and locked Cargo metadata resolves exactly one of each
and verifies the published files against the fork source.

## Relative production evidence

Two sequential pairs used the same authored Factory Site Layout Twin revision,
normal native desktop Avatar view, physical target, simulation settings and
matching diagnostic Tracy production builds. A disk-full startup and a
keyboard-driven camera movement were rejected before entering the cohort.
Each accepted comparison uses equally sized settled render-frame windows.
Other compilation workloads varied substantially; neither pair qualifies an
isolated whole-frame FPS claim. The second pair has more comparable concurrent
load and the following mean zone-work ratios:

| Native work | Candidate/reference |
|---|---:|
| Directional-shadow visibility | 56% |
| Camera CPU visibility | 85% |
| Both culling passes | 64% |
| Visibility pipeline (culling, reset, hidden admission, point lights, light extraction, collection) | 70% |
| Unchanged point-light visibility (load control) | 84% |
| Unchanged render visibility collection (load control) | 83% |
| Render schedule (observational) | 77% |
| Application throughput (instrumented, observational) | 117% |
| Application p95 frame time (instrumented, observational) | 75% |

Comparing culling ratios with the unchanged point-light control provides an
additional, workload-dependent comparison rather than an isolated speedup:
directional work is 67–77% of the control-adjusted reference across the two
pairs, and both culling passes together are 76–89%. Camera culling alone does
not establish an independent win in this workload: the second pair's median
camera zone is 105% of the reference. Same-thread sampled work in the second
pair is 65% for directional visibility, 118% for camera visibility, and 78%
for both together; collection, an unchanged control, is 85%. These samples
exclude culling work on parallel worker threads. The first pair had a
substantially heavier candidate workload, with unchanged systems also becoming
slower. The native camera cache can avoid a full pass on genuinely unchanged
or sparsely changing views, as the exact lifecycle tests prove, but a broader
candidate inventory still uses the bulk traversal. No clean uninstrumented FPS
or tail-latency improvement is claimed.

Both accepted actual-pinned pairs retained the same queried telemetry groups,
units and counts and reached ready simulation without runtime diagnostics.
Stationary viewport pairs agreed in over 99.99% of pixels. The moving-Sun
native graphics fixture passed all twelve changed-light phases with positive
green/cyan coverage and no red disagreement; the shared-shadow allocation
fixture also passed its authored verdict and pixel probes. The initial and
repeated capture receipts, traces, exact executables, relative tables and
quality gates are saved in `target/perf/incremental-visibility-20261126/`.
