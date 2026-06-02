# Compile-once + runtime params (Phase 2b) — feasibility scope

Goal: run a parameter sweep (Isp, crew, m_dry, …) re-using ONE compiled model
instead of recompiling per sweep point. Today each `Experiment` string-injects
overrides into the source and triggers a full rumoca front-end recompile
(~6s warm each ⇒ Isp×5 sweep ≈ 30–45s).

Investigated read-only across `rumoca/` and `crates/lunco-{modelica,experiments}`.
**Verdict: FEASIBLE with a small, well-bounded rumoca change. NOT blocked by
constant-folding in the way a first pass suggested.**

## What the interpreter actually does (corrects the codegen red herring)

lunica does NOT use rumoca's FMI/Python/Julia codegen templates. It runs the
**`rumoca-sim-core` interpreter** via `rumoca_sim::SimStepper` (concrete impl
`rumoca-solver-diffsol/src/stepper.rs`). Parameters are evaluated AT RUNTIME from
their `start` expression trees:

- `ic_solve.rs:457 build_params()` evaluates every `param.start` via `eval_expr`,
  in **two passes** to resolve forward references between parameters.
- So `v_circ = sqrt(mu/R_moon)` recomputes from whatever `mu` is in the env —
  derived params DO propagate, *as long as the expression survives to runtime*.

The stepper already carries the runtime machinery:
- `stepper.rs:158 param_values: Vec<f64>` — runtime parameter vector.
- `stepper.rs:159 input_overrides: Rc<RefCell<HashMap<String,f64>>>` — override map,
  read in `build_env()` (`stepper.rs:238-243`).
- BUT `set_input()` (`stepper.rs:187`) **rejects anything not in `input_scalar_names()`**
  — it's inputs-only; there is no public `set_parameter` and no `reinit`.

## The one real blocker: the fold collapses *computed* derived params

`rumoca-phase-dae/src/fold_start_values.rs` (`fold_start_values_to_literals`, run
unconditionally at `lib.rs:310`) constant-folds `start` expressions to literals.
It has a deliberate carve-out (`fold_start_values.rs:72-86`):

> "Parameter refs preserve the dependency so downstream overrides flow through to
> dependents instead of being locked at compile time."

…but that carve-out only preserves **bare top-level `VarRef`** starts (`v_circ = mu`).
A **computed** expression (`v_circ = sqrt(mu/R_moon)`, `dv_hop = 2*v_bo*(1+loss_frac)`,
`massRatio = exp(dv_hop/(Isp*g0))`, `propPerHop = m_dry*(massRatio-1)`) is not a bare
VarRef ⇒ it gets folded to a literal in the DAE the stepper receives.

AbdulezerPair.mo is almost entirely computed derived params. So today, overriding a
base param at runtime would NOT propagate to its computed dependents — which is
exactly why the current code recompiles from source instead.

Also note: `sort_parameters_by_start_deps` is referenced in the fold comment
(`fold_start_values.rs:79`) but only `sort_algebraics_by_equation_deps` is actually
defined — parameter dependency topo-ordering for forward eval appears NOT implemented.

## Minimal change set

### rumoca (needs explicit go-ahead — upstream edit)
1. **Extend the fold carve-out** so `dae.parameters` `start` expressions that
   *reference other parameters* are preserved symbolically (not just bare VarRef).
   Keep folding constants/states (and params with no param-deps) so codegen
   backends are unaffected. Cleanest: gate the parameter-fold so it preserves any
   start expr whose free vars include a parameter; literal-fold the rest.
2. **Implement `sort_parameters_by_start_deps`** (topo order on param→param deps)
   so the preserved expressions evaluate in dependency order. `build_params`'
   existing 2-pass loop already tolerates forward refs, but topo order makes it
   robust for deeper chains (AbdulezerPair has 3-deep: mu→v_circ→…→massRatio→propPerHop).

   With (1)+(2), the cached DAE keeps the derived-param expression trees, and
   `build_params` recomputes them from any overridden base — no stepper API change
   strictly required.

3. *(Optional, larger)* Split `build_stepper` into reusable structural-prep + cheap
   per-param init, and add `SimStepper::set_parameter` + `reinit`. Only worth it if
   `SimStepper::new` itself proves slow vs. the front-end. **Likely unnecessary** —
   see cost note below.

### lunco (no go-ahead needed; depends on rumoca #1/#2 landing)
4. **Cache the compiled `Dae`** per `(doc, class, source-hash)` in the runner
   (replaces `apply_overrides_to_source` + recompile in
   `experiments_runner.rs:829+`). On cache hit, skip the rumoca front-end entirely.
5. **Per sweep point**: clone cached DAE → set each override base param's `start`
   to `Literal::Real(value)` → `SimStepper::new(&dae, opts)` → integrate.
   `build_params` recomputes derived params from the overridden bases (given #1/#2).
   `dispatch_experiment` (`compile.rs:1119`) and the runner loop change from
   "mutate source + compile_str_multi" to "mutate cached DAE + new stepper".

## Cost note — where the time actually goes

The ~6s warm "compile" is the rumoca **front-end** (parse → flatten → to-DAE →
structural matching/BLT). `SimStepper::new` (IC solve + kernel build) is comparatively
cheap. So **caching the DAE captures most of the win even if we rebuild the stepper
per run** — which is why change #3 (split build_stepper) is probably not needed for
v1. Measure `SimStepper::new` alone on AbdulezerPair before investing in #3.

## Net assessment

- Not the "rewrite codegen / 2-3 weeks" picture an FMI-template reading implies.
- Real work = **one focused rumoca fold change (#1) + a small topo sort (#2)**, then
  a **DAE-cache refactor in lunco-experiments (#4/#5)**.
- Derived-param correctness is the crux and is handled for free by the existing
  runtime `build_params` once the fold stops collapsing computed param expressions.
- Risk to watch: codegen backends (FMI/Julia/SymPy) still want literals — gate the
  fold change so only the sim/interactive path preserves symbolic param exprs.

## Key file references
| What | File:line |
|---|---|
| Override → source string-inject (today) | `lunco-modelica/src/experiments_runner.rs:829+` |
| Experiment dispatch / compile | `lunco-modelica/src/ui/commands/compile.rs:1119` (`dispatch_experiment`) |
| Runtime param eval (2-pass) | `rumoca-sim-core/src/ic_solve.rs:457` (`build_params`) |
| Constant-fold pass + carve-out | `rumoca-phase-dae/src/fold_start_values.rs:14,72-90`; invoked `lib.rs:310` |
| Missing topo sort | `fold_start_values.rs:79` (referenced), `:333` (`sort_algebraics_…` only) |
| Stepper param vector / override map | `rumoca-solver-diffsol/src/stepper.rs:158,159,238-243` |
| Inputs-only setter (no set_parameter) | `stepper.rs:187` (`set_input`) |
