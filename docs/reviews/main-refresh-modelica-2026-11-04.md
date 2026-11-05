# Modelica integration and measured optimization review

The optimization checkout is fast-forwarded from `d6379130f` to local main
`a75323b6ac8d91c82df8d0898a610dca7e22f5a2`. The restored work preserves the new
reset target, source-library leases, in-flight step admission and layered-input
authority. No commit or push is part of this integration. Backup and retained
stash identity are recorded in `.cache/perf/main-refresh-slots-20261104/`.

## Execution owner

The integrated production startup exposed interleaved deferred compile requests
for the same participant. Adjacent-only deduplication admitted repeated solver
construction and failed coupled initialization's exact-session check.
`lunco-modelica-worker` now coalesces each exact source/participant/class across
other participants while preserving first-admission order, all listeners,
forced intent and the latest ordered run intent. A class change for the same
participant remains an ordered replacement boundary. The authoritative contract
is [architecture 20](../architecture/20-domain-modelica.md).

This is execution admission, with the worker as owner and the existing compile
queue as consumer. It is an engine invariant; no scenario policy hook is needed.
Exact source leases, retirement, replacement and certification failures retain
their existing owners. The new unit seam covers interleaving and class replacement;
the production Griffin run certifies all six coupled initialization participants.
No certification condition, finite check, clock, model equation or quality
setting is relaxed. The typed binding architecture remains unchanged.

## Verification

The focused integrated lib/tests compile check covers the worker, UI and USD
co-simulation owners. Worker execution tests pass 90 cases, with one manual
benchmark ignored. Two clean-build persistent-artifact tests are excluded:
the dirty Rumoca identity intentionally disables persisted compiler artifacts;
the initial unfiltered run records both refusals. The reset publication fixture
uses the installed target identity required by the current reset contract.

The production release passes 264 authored checks: 46 command/input/parameter,
20 checked lifecycle, 10 variable projection, 47 initialization/clock/receipt,
35 compiled metadata, 57 plume and 49 third-through-sixth Modelica lifecycle
checks. Two additional dedicated expected-fault scenes pass. These use the
production binary through owned APIs and authored Rhai verdicts. All owned
processes and ports close. Bundled and external model source hashes are preserved.

The command reference is regenerated from the owned windowed schema (306
commands) plus ten feature-specific commands from retained network schema
evidence. Shared definitions match exactly and every feature-command owner is
unchanged since that evidence. The resulting reference contains the current
316 commands. This is not a fresh networking runtime test.

## Current performance boundary

Normal release SHA-256:
`4b741375120f596da8a24148f2357f5365286bd30cff1a36674c9426fd4fd921`.
The High/2560x1440 Griffin run reaches first scene readiness in 14.1597 seconds,
completes terrain and physics/handoff admission, advances the authoritative
clock and finishes with zero runtime errors. Its 453-frame window gives
44.8323 FPS, 21.7744 ms median and 28.2270 ms p95. The 150 FPS budget fails.
MainPropulsion cumulative worker service averages 1.9818 ms over 2,283 jobs;
AttitudeActuation averages 2.0641 ms. Warm aggregate Modelica attribution has
median 5.4211 ms per physics tick; this is summed worker work, not barrier latency.
The 1 ms target is not achieved.

Main changed `LunCo.Actuation.WrenchAllocator` to consume the authoritative
center of mass. Its Gram matrix is now live algebraic state rather than a
fixed parameter matrix. AttitudeActuation exposes 1,059 variables versus 842
in the previous cohort. The current source therefore adds runtime computation
and removes parameter-only specialization. Earlier timing and Tracy samples
are a different generated workload and cannot establish this revision's
speedup or exact new CPU attribution.

A trace-interest hoist was evaluated against independent native executables.
Five alternating 3,072-row, 1,024-refresh trials give medians 6.355458 and
6.334449 ms, a 0.33% difference within the observed variation. No SDK edit is
retained: all 3,154 native source-manifest entries match the admitted baseline.
This evidence prevents treating a profiled trace-filter hotspot as a demonstrated
clean-run improvement. Existing parameter-only refresh certificates do not
justify treating a live COM input as immutable. Reuse of input-dependent
algebraic work needs an owner-issued dependency certificate, exact input bits
and parameter/session invalidation, with time/state/event/effect dependencies
excluded or explicitly represented. That extension is not implemented here.

All receipts, independent executables, timing samples, logs, source checks and
schema provenance are under `.cache/perf/main-refresh-slots-20261104/`.
