# Rumoca torn-sweep workspace proposal

Status: dependency authorization pending. No dependency source is modified.

The normal application pins `LunCoSim/rumoca` at `b825929bf32cc44797eb35a55d52d8d8af636569`.
The adjacent `../rumoca` checkout has that same clean head. Application geometry
executor work is independent of this proposal.

## Evidence and owner

The source-matched Summer capture uses owned PID 2811282, API 5306 and the
adjacent Tracy 0.14.1 tools with 1 kHz sampling. Its final 240 completed
`FixedMain` instances exclude 20 screenshot/shutdown ticks and pass 15 native
duration/counter checks. Framing and native exports pass. The selected
`modelica_integrate_macro_step` intervals contain 231 CPU samples, with no
missing stacks; 35 leaf samples resolve to `_mi_page_malloc_zero`. Those leaves
establish allocation cost, not attribution of every allocation to one caller.
Native algebraic refresh, compiled residual and Jacobian calls also appear.

`rumoca-solver/src/runtime/solve_runtime/refresh_projection.rs` owns the torn
algebraic sweep. In the pinned source:

- `torn_block_sweep` allocates raw residual storage and, with development checks,
  copies the incoming solver vector.
- `eval_compiled_torn_sweep` allocates a dense vector whose width is retained
  in the immutable compiled sweep.
- `debug_assert_torn_sweep_agrees` independently copies that incoming vector
  and allocates reference residual storage before replaying the per-row path.

These calls repeat during projection/Newton work and integration. The native
runtime already owns retained scratch buffers for other refresh operations;
this change should follow that ownership and allocation-failure discipline.
The application cannot control these internal buffers through the public ME
session API. Copying the native solver into application code is inappropriate.

## Proposed scope

Reuse typed state and residual workspaces at the existing runtime/prepared-sweep
owner. Refill all live entries at every call; retain capacity, never evaluated
results. Check nested evaluation and distinct runtime lifetimes before choosing
the exact borrow boundary. Do not share mutable storage between components or
introduce a cache of numerical answers.

Retain every independent per-row agreement check, decline decision, finite-value
diagnostic and full bitwise comparison. Preserve f64 arithmetic/order, native
execution, initialization, step lattice, solver tolerances and debug checks.
No profile change or check suppression is proposed.

Required qualification:

1. Native inline torn-block tests covering changing inputs, singular decline,
   repeated calls, independent components and failure after warm workspace use.
2. Exact complete solver-state/residual agreement and a bounded warm allocation
   diagnostic against the unchanged owner. Measure the proposed scope; the
   sampled allocator proportion is not a promised gain.
3. The maintained native canary, then application exact reference replay and
   a normal Griffin readiness/landing comparison. Report startup and settled
   effects separately, keeping raw profiling data ignored.

Publishing and pinning require a tested commit. The existing permission for
native factory, proof-key and immutable-index work does not cover this new
workspace change. `AGENTS.md` requires explicit permission before modifying or
publishing a dependency patch.
