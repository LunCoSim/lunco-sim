# Lunar Ops solver preparation

Measured on 2026-09-30 with the production
`assets/scenes/luncosim/lander_ops.usda` scene, High rendering quality, and
the authored physics settings. Other simulator workloads remained active;
these measurements are contention-affected diagnostics, not performance
acceptance.

## Owner and change

Tracy attributed the main startup delay to prepared-solver lowering of the
12-nozzle `AttitudePropulsion` Modelica network. Source compilation took
0.563 s; lowering took 62.557 s in that instrumented capture. Aggregate Avian
projection took 5.543 ms. The physics admission hold waits for the required
Modelica participants to initialize.

`LunCo.Propulsion.computePlumePhotometry` owns the stateless plume equations.
`PlumePhotometry` exposes its named USD ports, while `RCSJet` calls the same
function directly with static nozzle parameters and delivered engine signals.
The thruster equations and existing public RCS outputs retain their contract.
Rust, Rumoca, admission rules, precision, and physics settings are unchanged.

## Unprofiled cold preparation

Existing Rumoca stage timers supplied the lowering breakdown. Each run used
an empty prepared-solver cache. The baseline was `eaeef7608`; the final run
used the integrated `05d517863` tree with the model changes.

| Metric | Baseline | Final |
| --- | ---: | ---: |
| RCS algebraic variables | 468 | 156 |
| RCS solver lowering | 49.798 s | 33.137 s |
| Observation substitutions | 19.519 s | 12.373 s |
| Visible-expression substitutions | 8.438 s | 2.741 s |
| Stable readiness from process launch | approximately 58.96 s | 43.86 s |

The final lowering was approximately 33% shorter. Startup still spends tens
of seconds preparing equations on a cache miss. A previously measured warm
baseline reached stable readiness in 5.76 s; warm and cold runs are different
cache conditions.

After final admission, 240 retained fixed ticks had service-time p50 4.984 ms,
p95 7.022 ms, p99 8.043 ms, and maximum 9.778 ms. This is whole fixed-tick
service, not Avian-only time. Concurrent workload differences prevent a
controlled running-performance comparison.

## Verification

- `cargo build -p lunco-luncosim -j 4` passed on the integrated tree.
- Production `rcs_feed_starvation` passed: 14 checks, including numeric
  thrust, flow, luminous power, envelope, and zero-feed negative cases.
- Production `rocket_engine_plume_defaults` passed: 11 checks covering
  defaults, scaling, fuel/richness colour, and zero-flow visibility.
- The final headful scene reached readiness and loaded both Earth and Moon
  imagery. Its private cache contained physical texture/font copies within
  the owning root.
- Final owned sessions used API ports 4163, 4164, and 4165; all exited.

Local evidence: `/tmp/lunar-ops-function-20260930/final/` contains logs,
readiness samples, phase timings, and runtime metrics. The original Tracy
capture is `scripts/perf/captures/lunar-ops-startup-20260930.tracy`.
