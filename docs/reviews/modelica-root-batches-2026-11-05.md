# Modelica root dispatch qualification

The optimization checkout remains at `a75323b6a`, preserving its existing
application and SDK work. This change belongs to Rumoca's generic solver owner;
LunCoSim pins the SDK work to local Rumoca commit
`a11b4b2e0adf52818a711af945c7bcbbfbf5c184`. The recorded production
validation consumed the same source through its local build override. Both
repositories remain unpublished; independent machines need that SDK commit
before resolving the application pin. No application math adapter, model equation,
timestep, substep, precision or rendering-quality change is introduced.

## Analysis and owner

The fresh diagnostic executable is
`bc10502b9aba955c9619978b609ab4b65e855e8b687e7274e131d32f5a553f62`.
The admitted capture uses the production `--no-ui` realtime host, an owned API
port and Tracy 0.14.1 at a verified 1,000,000 ns sampling period. Native framing
and CSV/sample exports succeed. The saved 225,321,663-byte capture has 14,199
blocks and 225,264,861 payload bytes, matching the collector's 214.83 MiB report.

In its final ten seconds, 376 of 2,619 selected integration leaf samples land in
`try_compiled_program_outputs`; native expression wrappers account for additional
leaf samples. These are sampled CPU scopes across all workers, with incomplete
caller recovery through anonymous JIT code, not exact whole-frame attribution.
The count-10 cohort averages 1.3384 ms integration and 0.4100 ms observation per
job. Both count-6 participants share a cohort; that marker alone does not identify
AttitudeActuation exclusively.

The root plan already owns fixed public/search selections. Rumoca now prepares
those selections into checked complete-program batches, preserving source order,
output identity, current coordinates and error boundaries. No input-dependent
result is cached. The SDK's [root execution contract](../../../rumoca/docs/dev-guide/src/runtime/simulation-runtime.md#root-condition-execution)
and [owner review](../../../rumoca/docs/reviews/root-condition-batches-2026-11-05.md)
record construction, lifetime, native failure and verification semantics.

## Source cohort and graphics boundary

The Twin's dirty default manifest points to a film plan while its Git HEAD remains
`92094315c59bdb7478df77073ecfb698d423dc69`. Measurements explicitly open
`twins/astrobotic-griffin-1/scenes/griffin_1_surface_ops.usda` through `OpenFile`.
Each admitted headless window retains readiness, physical handoff, advancing
physics ticks, thirteen participants, complete variable inventories and zero
runtime errors. All 546 external source-file hashes match across the four compared
runs, including the 1,059-variable AttitudeActuation and 3,035-variable
MainPropulsion workload. Bundled model sources remain unchanged.

The initial graphical diagnostic selected the changed Twin default and hit a
structured out-of-memory presentation fault while another renderer and a video
player were active. That run is rejected. Only this task's process was closed;
foreign sessions were not controlled. The headless measurements qualify CPU and
co-simulation work. They establish no graphics/FPS gain or 150 FPS acceptance.

## Production measurements

Reference normal executable:
`4b741375120f596da8a24148f2357f5365286bd30cff1a36674c9426fd4fd921`.
Candidate normal executable:
`cf990754b7894ef6e93a127ba1327d818075dae0b71f50b91e045dcc2219de06`.
Both are independent executable snapshots; the candidate is installed at
`target/release/luncosim`.

| Worker scope | Reference A | Candidate A | Reference B | Candidate B |
| --- | ---: | ---: | ---: | ---: |
| AttitudeActuation cumulative mean ms/job | 2.3895 | 0.3613 | 2.9588 | 0.8772 |
| MainPropulsion cumulative mean ms/job | 2.1610 | 1.8843 | 2.8812 | 5.6412 |
| Warm aggregate worker ms/physics tick, median | 6.1114 | 3.0653 | 6.8420 | 10.7364 |
| Warm largest-participant mean ms, median | 2.3548 | 1.3770 | 2.6132 | 5.2926 |

Candidate A reduces AttitudeActuation service by 84.9–87.8% and aggregate work by
49.8–55.2% against the two references. Candidate B encounters active foreign
compilation and substantial memory pressure: AttitudeActuation remains cheaper,
but MainPropulsion and aggregate service regress. These four windows do not
establish a stable whole-Modelica gain. The dispatch mechanism is qualified by
its call-count regression and alternating native fixture; the observed production
improvement is conditional on load. Every concurrent workload is retained in its
receipt. Worker service attribution is not coupled-step wall-clock latency, and
parallel service can overlap. Startup readiness also varies; no startup gain is
established.

A further run admitted after a compiler-free low-pressure interval is excluded:
the Twin manifest and a film scene changed during measurement. Its source guard
rejects it, and its owned process/port close. Live external edits and memory
pressure remain explicit limits on a clean production acceptance window.

MainPropulsion is now the largest participant. The 1 ms and 1–2 ms aggregate-work
targets remain unmet. Model source, physics cadence and authored behavior are
preserved.

## Verification

The solver passes 562 unit tests, including root selection cardinality, admitted
native error propagation, grouped/sparse/duplicate ownership, signed zero and
invalid-buffer rejection. Five alternating independent native fixture pairs
measure 11.938011 versus 0.822214 ms for 384 roots over 512 fresh times (93.11%).
That fixture timing is separate from production worker service.

The fixed 20-model canary preserves all classifications and twelve strict-high
trace comparisons. All 562,411 values and 16,841 timestamps are bit-identical;
six ToDAE and two solve-IR refusals remain explicit. This is not full-cohort parity.
Production passes 264 authored checks plus two dedicated expected-fault scenes,
including initialization/clock/receipt, variable projection, input authority,
compiled metadata, plume and the third-through-sixth Modelica lifecycle cases.
Clippy retains three pre-existing nesting errors in unchanged solver locations;
no new warning or suppression is introduced.

Evidence is `.cache/perf/modelica-current-20261105/`: source manifests, independent
executables, fresh native capture and exports, regression proofs, fixture pairs,
canary delta and owned API/session receipts. The work is committed and integrated
locally at the user's request; no push is included.
