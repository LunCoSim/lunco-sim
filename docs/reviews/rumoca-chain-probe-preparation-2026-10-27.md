# Rumoca output preparation

The approved local dependency change lives in the adjacent
`rumoca-griffin-loading` worktree, branch `fix/griffin-loading`. Application
workspace dependencies resolve to that worktree. Nothing has been published.
The implementation and current performance evidence are tracked in
[Griffin loading budget](griffin-loading-budget-2026-10-30.md).

## Owner and contract

`rumoca-eval-solve::PreparedScalarProgramBlock::torn_sweep_runs` discovers run
membership by incrementally extending the same checked, source-bound candidate
used by executable chains. Each appended step is materialized once; a declined
extension starts a fresh candidate. Executable consumers invoke shared-value
optimization only after admission.
Target dependencies, failable operations, prefix order, register materialization,
final equivalence proofs and runtime agreement checks remain authoritative.

Output-only chain and directional-JVP consumers use `PreparedEvaluationBlock`.
Projection and state-manifold Jacobians use that type for whole and selected
output evaluation as well. Assignment certificates remain with source residuals;
output consumers cannot query certificates they do not own. Alternate charts
reuse the existing checked replacement-program mechanism.

These are immutable compiler/evaluator mechanisms, not application lifecycle
policy. No new Rhai hook, simulation-state cache or alternate solver is involved.
The canonical dependency specification is `SPEC_0043_CONSTRUCTION_CATALOG.md`.

The canonical assignment constructor first asks the existing checked structural
dependency owner whether the output can depend on the candidate target. A proven
absence rejects that target before deriving or materializing its isolator.
Unbounded dependency sets keep their ordinary candidate checks. This prevents
a tensor range from issuing certificates for unrelated output coordinates.

Within one `SolveRuntime`, exact refresh sequences with identical ordered
compiler-issued program IDs share one checked segment preparation and native
compilation result. The first issued sequence identifies that shared result.
Identity reuse is scoped to the same immutable model and runtime; independently
constructed runtimes retain separate preparation and live scratch.

`rumoca-exec-cranelift` groups independently selectable residual and Jacobian
programs into bounded native dispatch functions. Each arm retains its own
register and value state and executes only the requested program. Grouping
reduces function construction, cache lookup and relocation work; it does not
change equations, output placement, input validation or numerical execution.
Shared conditional owners retain their complete-call boundary, and large
residual programs retain bounded chunk emission.

Structural dependency propagation shares immutable ordered sets through its
existing register walk and refresh dependency reader. Unions detach before
mutation; owned public results retain their original contract. This removes
repeated tree copies during checked prepared-model reconstruction without
changing wire data, dependency rules, provenance or rejection of invalid input.
The local dependency review and fixed-canary comparison are in
`docs/reviews/shared-dependency-sets-2026-11-01.md` and
`docs/reviews/shared-dependency-sets-canary.json` in the adjacent Rumoca worktree.

## Qualification

The native chain fixture covers 64 ordered outputs and exact evaluated values.
Existing tests cover split chains, target dependence and failable operations.
Evaluator tests assert that output-only blocks derive no assignment certificates
and preserve whole/selected tensor outputs. Solver tests exercise projection,
sensitivity, initial continuation and manifold behavior.

Product acceptance requires a normal production Griffin run through physics
readiness, landing handoff, diagnostics, visual inspection and owned-session
closure. Profiler timings and native microbenchmarks do not establish the
five-second product budget. The current results and validation limits belong in
the loading-budget review.
