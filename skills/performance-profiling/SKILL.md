---
name: performance-profiling
description: Diagnose or improve LunCoSim FPS, physics time, periodic stalls, Builder versus View differences, terrain/render cost, or Tracy captures. Use when performance must improve architecturally without changing BigSpace, substeps, shadows, terrain quality, or image fidelity.
---

# Performance profiling: measure the owner, then remove avoidable work

Read [`scripts/perf/README.md`](../../scripts/perf/README.md) and the current
open handover in [`docs/reviews/open-400fps-performance-handover.md`](../../docs/reviews/open-400fps-performance-handover.md)
before changing code. A status-bar FPS number is a symptom, not an attribution.

## Required separation

- Run one clean production session for the product FPS number. Do not use
  Tracy's instrumented number as acceptance evidence.
- Run a separate Tracy build/capture using the adjacent `../tracy` checkout;
  start `tracy-capture` before the production binary and inspect the settled
  window, not only startup.
- Compare Builder and View with the same scene, camera, rendering-quality
  settings, physics substeps, shadow settings, and terrain assets. A Builder-
  only cost usually means an editor observer, rebuild, projection, or UI path,
  not that physics needs a different global timestep.
- Attribute CPU, GPU, physics, terrain, and UI separately. Never disable
  shadows, lower authored terrain quality, change BigSpace, or change the
  standard substep count to make a graph look better.

## Architecture checks

Search the owning systems for unconditional writes, full-set topology scans,
repeated observer registration, per-frame allocations, polling, and work that
should be gated by a revision/change event. Structural edits should invalidate
structural caches; transform propagation and telemetry output are not by
themselves topology changes. Check both the Builder and View registration paths
before fixing only one.

Treat Bevy `Changed<T>`/`Added<T>` filters as population filters, not free
events: a no-match query can still inspect candidate entities, and separate
`is_empty()` queries can repeat that work. Combine compatible invalidation
sources into one `Or` query when they drive the same decision. If this remains a
hot path, audit every writer before adding a source-owned event/revision/dirty
set; once all writers are accounted for, prefer that signal over an
always-evaluated `Added<T>` population query in a run condition.

For deferred USD projectors, keep one entity-work set per owner and feed it
from the complete lifecycle boundary: identity arrival, projection readiness,
invalidation, removal, and scene teardown. The same applies to deferred adapter
steps such as wrapping a Modelica model into its shared port surface. Keep a
single bootstrap discovery for entities predating plugin installation, and
retry only work whose authoritative stage/readiness input is still pending.
The idle run condition should inspect the owner set, not scan the population.

For whole-index projectors such as USD telemetry, use one initial bootstrap,
then coalesce relevant insert/remove observers into an invalidation flag. Keep
stage-generation and asset-store invalidation as scalar checks. Do not repeat
the same `Added`/`Changed` population filters in both the run condition and the
projector, and do not reproject until the index is invalidated.

When a USD reader already exposes `has_authored_attribute`, use it to test one
known property instead of enumerating every attribute name. If one enumeration
feeds multiple derived port sets, derive them together from that single result.
Temporary lookup indexes over immutable ECS queries should borrow path and port
surface data instead of cloning those maps for a one-pass reconciliation.
Build compatible per-entity indexes in one query traversal rather than running
separate full-population passes for each index.
For derived marker sets, compare current membership with the desired set and
apply only additions/removals; unrelated rebuilds must not emit lifecycle churn.
Removal invalidation should be qualified by the entity's authored USD identity
and relevant endpoint capability, not by a generic component removal alone.

Keep invalidation domains distinct: a wiring/topology latch may be raised by
endpoint arrivals and must not automatically trigger whole-stage domain
discovery. Use the canonical stage generation or authored-stage asset change
for that broader pass; keep entity arrivals on their queued-entity path.
Generated-source document sync should share the `PendingEntityWork` contract,
and its metadata publisher should consume the owner-published dirty flag rather
than adding a parallel change query.

When gating a dependency's multi-system transform schedule, distinguish its
per-update output flags from authoritative input changes. Preserve any required
settle pass after a floating-origin change, and test the change → settle → idle
sequence so a gate neither stays open forever nor closes before propagation.
Keep the settle marker between admitted passes so the idle admission check does
not replace expensive propagation with a full-grid scan.

For physics, distinguish persistent environmental state from transient
commands: use the engine's persistent acceleration or passive non-waking force
contract for gravity and contact support, and reserve waking force/torque writes
for authored drive, braking, or actuator commands. Do not emulate this with a
timer, sleep threshold, or a second cache.

Use the existing cache/revision owner and preserve the USD projection boundary.
Do not add a second cache, a timer, a compatibility path, or a quality fallback.
If the hot path is not established, stop after source inspection and capture a
bounded profile rather than guessing.

## Evidence

Record the clean FPS window, physics and render timings, Tracy capture path,
scene/settings, and whether the result is startup or settled. Rebuild the
production binary after a code change and repeat one clean A/B plus one Tracy
diagnostic capture. Link the changed owner and state any platform/GPU evidence
that was not available.
