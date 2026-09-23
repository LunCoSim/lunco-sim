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
set; do not create another per-frame full-population scan.

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
