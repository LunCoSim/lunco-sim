# Open terrain preparation and native visibility findings

Scope: current terrain and presentation audit, including the retained Bevy
camera/light visibility revision `87cb795e3f11da0d769d28e57df0ec42a7112aa8`.

## V1 — retained visibility must account for entity disabling

**High; source review, not runtime-confirmed.** Bevy's default queries exclude
`Disabled`, yet the retained camera and directional-light culling owners do not
include add/remove of that component in their sparse invalidation inventory
(`crates/bevy_camera/src/visibility/mod.rs`, `crates/bevy_light/src/lib.rs` in the
pinned fork). A disabling transition with unchanged transform, visibility,
layers and bounds can leave a visible entity in a retained list; re-enabling
an entity can omit it until a separate invalidation or full pass. Existing
application receiver-crop code explicitly observes both `Added<Disabled>` and
`RemovedComponents<Disabled>`, demonstrating the lifecycle boundary.

Repair belongs in the maintained Bevy owners, not in a LunCoSim cache-clearing
shim. Test a stationary camera and directional light with sufficiently many
unchanged candidates to exercise the sparse path, disable and re-enable one
visible candidate, assert exact class/cascade lists, retained union and
`ViewVisibility`. An upstream change and publication require separate explicit
authorization.

## T1 — transactional mesh ceiling can prevent convergence

**Medium.** The mesh admission guard handles a single allocation larger than
the entire cap. A replacement frontier may also be structurally impossible to
admit when its outgoing draw partition must remain resident and the union of
required current meshes exceeds the cap. The current pressure status does not
distinguish that case from transient cache pressure. Preserve the outgoing
cover and report a terminal exact working-set requirement only when all
required live and replacement identities can be accounted for without double
counting. Test a pressured transition and recovery when the cap changes.

## T2 — settled-cover accounting and focus lookup

**Medium; unprofiled.** `collect_terrain_detail_demands` and the streamer both
scan the selected cover in settled frames; focus readiness also scans leaves.
Before retaining a computed status, enumerate all resident/selected mutation
boundaries (including depth reset and pruning) and invalidate the status there.
For focus lookup, use the existing quadtree coordinate/ancestor lookup with
explicit shared-edge and outside-footprint semantics. Measure the affected
zones before claiming throughput gains.

## T3 — expensive final raster preparation

**Medium; measured wait, not isolated CPU profile.** A 2048² final raster
with 16 directions and 16 AO steps can evaluate up to 268 million source
samples in the half-resolution AO stage. The cold workshop run published its
interactive preview at 32.51 s and final preparation at 93.34 s on the
integrated audit build; the previous run took 124.12 s under different shared
machine load. The invariant ray offsets are now prepared once; an exact output
test passed, but these timings do not establish an isolated speedup. Do not
lower authored quality. After measuring the owning bake,
consider bounded deterministic row partitions of the immutable field and
assemble them at the current source lease; do not synchronously join work on
the render or simulation schedule. Queued and completed obsolete derived work
is now retired correctly; a running derived raster still computes to completion
before its worker permit is released. Row partitioning should also make that
expensive kernel cooperatively cancellable.

## T4 — private render-layer retirement

**Medium.** The layer allocator still holds released slots for a fixed number
of frames. Exact reuse requires a fence for surviving `SceneViewGeometry`
entities in that layer after demand retirement, together with the render
extraction contract. Do not simply remove the delay while retired geometry
can still reference the slot. Validate with rapid close/reopen of independent
feeds and native extraction.
