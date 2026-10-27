# Deterministic native joint passes

## Owner and failure

Avian owns the XPBD joint correction passes. Each correction changes the shared
solver-body state consumed by subsequent joints. ECS query table packing must
therefore not determine their execution order.

Application `8bad5b08b`, pinned to Avian `95d02b3`, reproduces the unchanged
articulated replay failure on the fourth bounded native diagnostic run (PID
336008/API 5511). The first three runs pass. The production verdict identifies
Rover01 at tick 11 with zero tolerance. Native traces locate the divergence in
the first positional pass, before contact.

That pass contains 16 revolute joints. By endpoint source path, joint definitions,
prepared constraint data, inverse masses/inertias and timestep match between the
passing and failing runs. Within Rover01 and Rover02, traversal reverses from
bogie-left, bogie-right, rocker-left, rocker-right to the opposite order. Inputs
to subsequent shared-body corrections diverge immediately. Rover03 and Rover04
retain their order and state. This establishes the native query-order defect
independently of Modelica workspace reuse.

## Repair contract

The common native XPBD pass traverses each joint type in ascending live entity
index using Bevy's sorted query iterator. Position, velocity preparation and
velocity projection consume the same order. Joint preparation remains independent
per constraint. Equations, precision, substeps, iteration counts and references
are unchanged. The native mechanism also covers custom XPBD constraints using
this pass without requiring membership in a separate registry.

Velocity relaxation sorts and collects borrowed query items once per call,
then reuses them through its fixed iteration loop. The component borrow prevents
structural invalidation during that call; no order cache survives the call and
no new lifecycle registry is needed. Position solving uses the same ordering.

Entity indices uniquely identify the live slots used here. Their order is
independent of ECS archetype packing; cross-session replay still requires
deterministic entity creation and topology admission by the application. This
does not introduce a persistent source identity or cross-platform guarantee.
Application-owned ordered joint admission remains necessary.

This is numerical execution order at the solver owner, not changeable scenario
policy; no Rhai hook is appropriate. Production Rhai retains the replay verdict.
The native regression compares full body-state sequences after an unrelated
component move and after joint disable/re-enable. It also verifies that each
perturbation actually reverses query table order.

## Validation and publication

The user approved instrumentation, the confirmed repair, tests and publication
on October 27. Work uses `../avian-joint-replay` on
`fix/deterministic-joint-passes`, based on the application's `95d02b3` pin.
Unrelated joint-motor work and a dirty test file in `../avian` are preserved.
Native commit `ba8978bd647a5e9f911d9160219cc2f9c1795f83` is published on that
branch and verified against the remote. The application pins that Git revision;
no local path dependency remains.

The focused native f64 enhanced-determinism regression passes on the formatted
final source. Removing the ordering repair makes its unrelated-component case
fail, establishing a negative control. The regular published-pin build passes;
all ten unchanged production replay profiles pass 360 checks, with three extra
passes of the previously intermittent profile. The joint, native force-drive,
differential and continuous-contact scenes pass another 49 checks. Tests use
owned API 5515 and exit normally. The final receipt is
`portable-validation.json` in the ignored evidence directory below.
The reference SHA-256 remains
`6247db0dfefbd0eaedad3d712c359578f30a75addb6daea4a79584b784a70451`.
No tolerance or expected state is changed. Rumoca `fbfbcdc9` still requires
separate requalification against this native repair.

The application integration preserves main's `532243895` presentation-clock
repair. The rebuilt combined tree repeats all 360 exact replay checks, the 49
joint/contact checks, and the 13-check production presentation-clock gate on
owned API 5517. `integrated-validation.json` records the executable hash and
verdicts. The unrelated tracked handoff is restored byte-for-byte after rebase.

The integrated regular Griffin run (PID 750831/API 5518) preserves all 271
source hashes at model `2b3c0a9`, reaches four-pad handoff and sleeping finite
body state, and reports zero runtime errors or warnings. Rendering is inspected;
the process exits and its port closes. Its receipt and screenshot are under
`griffin-integrated/` in the ignored evidence directory.

Readiness takes 21.302 s; the initializer constructs with 16,482 native hits,
zero compilations and 3.397 s measured session construction. The latest native
physics history has median/p95 0.388/0.525 ms, while fixed-schedule service is
3.767/4.408 ms and MainPropulsion worker service averages 7.132 ms (last 5.504 ms).
Frame median/p95 is 9.967/13.055 ms. These distinct timing boundaries are not
additive. Desktop/browser activity is recorded, and no isolated whole-product
comparison is established. Loading, graphics and complete coupled-step budgets
remain open.

## Measured ordering cost

Two owned Tracy 0.14.1 captures run the unchanged 20-rover serial profile with
1 kHz sampling, identical logging filters, eight substeps and unchanged solver
iterations. Each has 780 physics ticks and 6,240 velocity-relaxation calls, and
passes the exact reference. Reusing ordered borrowed query items reduces that
phase's per-tick median/p95 from 1.897/2.212 ms to 1.237/1.359 ms compared with
sorting in every pass. Revolute position solving increases slightly from
0.109/0.126 ms to 0.116/0.139 ms. These are instrumented owner timings, not an
isolated whole-product speedup or a sub-ms coupled-step acceptance result.

Both captures pass official CSV export and complete compressed-block framing
checks (103,661,285 and 94,668,009 bytes). The captures and receipts identify
owned PIDs 633138 and 641807, API 5514, and Tracy port 8460; both sessions close.
Raw captures are ignored under `scripts/perf/captures/` with names
`joint-order-20-rovers-20261027.tracy` and
`joint-borrow-order-20-rovers-20261027.tracy`.

All diagnostic sessions closed. Temporary native tracing is removed. Raw
traces, normalized pass comparisons, build logs and test receipts remain ignored
under `.cache/codex-evidence/performance-20261027/joint-replay/`; profiling data
is not tracked. Builds use normal targets, regular sccache and four Cargo jobs.
