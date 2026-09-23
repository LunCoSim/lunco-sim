# Open — whole-simulation deterministic execution

**Reviewed:** 2026-09-23
**Scope:** USD lifecycle/projection, Modelica co-simulation, SysML analysis,
Rhai execution, Avian physics, and async preparation.
**Verdict:** The runtime has useful fixed-step and causal-barrier foundations,
but it cannot currently guarantee that async completion, untracked script
dependencies, or ECS query order leave the same authoritative state at each
tick. Stable actor/event/worker submission order is implemented; the whole
contract remains open.

## Findings

| ID | Severity | Finding and evidence | Status |
|---|---|---|---|
| D1 | P1 | Initial scene readiness holds physics/scenario execution but does not hold the authoritative tick: [`advance_sim_tick`](../../crates/lunco-core-runtime/src/lib.rs#L177) advances whenever `Time<Virtual>` runs; [`apply_physics_holds`](../../crates/lunco-physics/src/lib.rs#L853) pauses only `Time<Physics>`. Async load duration can therefore change the scene's first live `SimTick` and clock state. | Open |
| D2 | P1 | Referenced assets become live when asynchronous loads complete: [`drain_ref_spawns`](../../crates/lunco-usd-bevy-runtime-core/src/twin_projection.rs#L2057) authors and projects ready references, while the readiness owner does not include the full pending-reference closure. Runtime topology can be admitted at a completion-time-dependent tick. | Open |
| D3 | P1 | Rhai `get`/`port` and `set`/`port_set` access the live port registry directly ([`world_bridge.rs`](../../crates/lunco-scripting-rhai-world/src/world_bridge.rs#L1507)); [`derive_causal_barrier_participants`](../../crates/lunco-usd-sim-cosim/src/wiring.rs#L891) traces only declared `SimConnection`s. A script may consume a Modelica output without that dependency holding the shared clock. | Open |
| D4 | P1 | Production GUI physics is not admitted as deterministic: the composition root records [`PhysicsDeterminism::from_compute_threads(None)`](../../crates/lunco-luncosim-simulation/src/lib.rs#L392), and Avian's parallel implementation uses the shared compute pool. Fixed `dt` alone does not establish repeatable contact/constraint results. | Open; choose and measure a production profile |
| D5 | P2 | Rhai source/AST compilation and module top-level evaluation can run synchronously during a fixed scenario pass ([`scenario.rs`](../../crates/lunco-scripting/src/scenario.rs#L891), [`world_bridge.rs`](../../crates/lunco-scripting-rhai-world/src/world_bridge.rs#L3018)). Only immutable parse/compile preparation is safe to move off-thread; world initialization must retain a deterministic activation boundary. | Open |
| D6 | P2 | SysML analysis can synchronously discover/read a Twin source set and build semantic analysis ([`AnalyzeSysml`](../../crates/lunco-scene-validation/src/validate.rs#L815), [`sysml_analysis.rs`](../../crates/lunco-scene-validation/src/sysml_analysis.rs#L31)). A scenario query can therefore perform source I/O and parsing on the fixed hook path. | Open |
| D7 | P2 | Opening a USD Twin parses authored source and serializes the persistent overlay on the main schedule ([`drain_pending_twin_docs`](../../crates/lunco-usd-bevy-runtime-core/src/twin_projection.rs#L446), [`UsdDocument::with_origin`](../../crates/lunco-usd-document/src/document.rs#L982), [`persistent_composed_source`](../../crates/lunco-usd-document/src/document.rs#L1230)). Immutable source preparation should be revision stamped and off-thread; the live USD stage stays with its thread-affine owner. | Open |
| D8 | P2 | USD-to-Modelica projection extracts member/interface facts from source synchronously before worker compilation ([`dispatch_loaded_modelica_sources`](../../crates/lunco-usd-sim-cosim/src/lib.rs#L1455), [`ast_extract.rs`](../../crates/lunco-modelica-ast/src/ast_extract.rs#L143)). Move that pure extraction to source preparation and publish it with the matching source revision. | Open |
| D9 | P2 | The command journal does not yet provide a whole-simulation authoritative input log and replay verdict ([`command-journal.md`](../architecture/command-journal.md#L1)); adaptive Modelica also has an explicit non-bitwise-reproducible numerical profile ([`28-modelica-realtime-physics.md`](../architecture/28-modelica-realtime-physics.md#L26)). A precise guarantee must separate tick/order determinism from numeric cross-platform replay. | Open |
| D10 | Fixed | Scenario hooks and teardown previously followed ECS/hash iteration order even though synchronous `cmd()` can affect a later hook in the same pass. The driver now orders actors by `GlobalEntityId`, with a world-local entity tie-breaker. | Landed in current branch |
| D11 | Fixed | Telemetry events previously preserved observer arrival order and could hide an older event behind a newer tick in the FIFO queue. Eligible batches now use a stable typed total order before delivery. | Landed in current branch |
| D12 | Fixed | Connected USD events and serialized Modelica step requests previously followed ECS query order. Both dispatch paths now sort by scene identity before publication/submission. | Landed in current branch |

## Migration order

1. **Stable boundary order (landed).** Canonicalize Rhai actors, teardown,
   same-tick event batches, connected-event emission, and Modelica command
   submission. Production Rhai event-delivery coverage asserts that reverse
   enqueue order is delivered by the canonical event key.
2. **Async immutable preparation.** Move Rhai parse/import preparation, SysML
   source analysis, initial USD document parsing/overlay serialization, and
   Modelica interface extraction to revision-stamped workers. Keep Rhai top-level
   world initialization and live stage mutations on their owner schedules.
3. **One simulation-progress admission owner.** Add reason-keyed progress holds
   for initial readiness, runtime references, required Modelica results, and
   revision commits. Do not let load/compile time consume authoritative ticks;
   admit referenced topology only at a declared boundary.
4. **Close the dependency graph.** Make dynamic Rhai port access either a
   declared dependency in the causal graph or a typed read-snapshot/action-plan
   contract. Every authoritative read/write must have a producer, tick, and
   stable identity.
5. **Pin and validate physics execution.** Choose serial Avian execution or
   deterministic parallel operations after a measured production baseline.
   `PhysicsDeterminism` must reflect the actual pool/solver selection.
6. **Record/replay acceptance.** Add a typed authoritative input log and compare
   composed USD, Modelica, Rhai, SysML verification inputs, and Avian state at
   fixed tick boundaries. Record solver/build/platform scope in each verdict.
7. **Measure throughput.** After the ordering and admission contract is stable,
   use a settled production scene and a separate Tracy capture to verify that
   async preparation removes UI/fixed-schedule stalls without hiding solver cost.

Phases 2–6 remain necessary before claiming whole-simulation determinism. The
current ordering fixes do not close async activation, dynamic dependency, physics
numeric, or replay gaps.
