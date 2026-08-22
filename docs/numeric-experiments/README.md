# Numeric experiments — solver / model integration

This is the living solver/numerics reference: working configurations,
known-failing models, and the ranked rumoca / `lunco-modelica` backlog. Durable
solver findings belong here; the related rumoca workarounds are indexed in
[`architecture/29-rumoca-workarounds.md`](../architecture/29-rumoca-workarounds.md).

## Solver tuning reference — known configs, known-failing models, and the rumoca/lunco-modelica backlog

### Known working solver configurations

- **Stiff radiative thermal models** (lunar rover, anything with σT⁴
  networks + tanh hysteresis): `tolerance = 1e-3` (not 1e-6), `dt = 3600`.
  Scales linearly to multi-year horizons.
- **Solver choice — BDF is the robust default again.** The 2026-05-28 session
  concluded TR-BDF2 ("BDF dies at the second sunrise louver crossing"), but
  that predates the rumoca solve-IR fix for connection flow-sum unknowns. With
  that fix BDF completes the full multi-lunar-cycle horizon on the stiff
  thermal models, while the diffsol 0.13 SDIRK tableaus (TR-BDF2 / ESDIRK34)
  hit "nonlinear solver failures (50)" within the first lunar hour on the same
  models. SDIRK stays opt-in. (Source of truth:
  `crates/lunco-modelica/src/experiments_runner.rs` default-solver comment.)

### Known-failing models — don't waste time tuning solvers

These fail because of structural rumoca gaps, not solver-config gaps.
Picking a different solver or tolerance won't help; only the listed
upstream fix will.

| Model | Failure | Root cause |
|---|---|---|
| `Modelica.Blocks.Examples.PID_Controller` | bails at t≈2.85e-6 on every implicit solver | Uses `initType=SteadyState` + `initial equation der(spring.w_rel) = 0`. Both demand a homotopy/continuation IC solver; rumoca has plain Newton on FD Jacobian → degenerate y₀. Needs **homotopy IC** + ideally **symbolic Jacobian**. |
| `Modelica.Blocks.Examples.RealNetwork1` | rumoca returns `EmptySystem` at stepper init | Compile pipeline produces empty DAE for this model. Separate rumoca compile bug, unrelated to solver tuning. |
| `Modelica.Mechanics.Rotational.Examples.First` | advances to t≈0.073 then fails mid-event | Event-driven dynamics that need restart-after-event. Rumoca's event support is limited. Needs **event detection + state restart**. |

**Symptom that maps here**: bit-identical `fail_t` across solver/tolerance
sweeps means the failure is *deterministic in the solver's first few
steps* — that's the IC solve, not anything tunable from the FastRun API.

### Outstanding solver / numerics tasks (rumoca)

Priority ranking; each links back to the originating experiment report.

1. **Homotopy initialization for consistent-IC solve** (~½–1 week).
   Blocks `Modelica.Blocks.Examples.PID_Controller` and any model with
   `initType=SteadyState` or `initial equation` algebraic constraints.
   This is the most impactful single task right now — would unblock a
   large share of MSL examples that currently fail at IC.
   Origin: this same 2026-05-28 session (PID_Controller diagnosis).
2. **Symbolic Jacobian via rumoca AST + cranelift** (~weeks, highest leverage).
   Replaces finite-difference Jacobian which loses ~9 sig digits on radiative
   terms. Closes most of the remaining gap to OMC/DASSL on stiff models.
   Origin: the 2026-05-28 lunar-thermal session (report deleted; git).
3. **Per-state `atol` vector honoring Modelica `nominal=`** (~1 day).
   `SimVariableMeta.nominal` is parsed but ignored by the solver.
4. **`EmptySystem` compile bug** investigation. Trivial models like
   `Modelica.Blocks.Examples.RealNetwork1` lower to an empty DAE — rumoca
   compile-pipeline regression. No experiment report yet; needs a
   dedicated diagnosis session.
5. **Event detection + state restart**. Needed for models with
   relational `if`/`when` clauses (most physical-modeling models).
   Today rumoca treats events as smooth, which corrupts BDF history.
   Origin: `Modelica.Mechanics.Rotational.Examples.First` mid-sim
   failure.
6. **Tiered `SolverStartupProfile`**. Today's aggressive defaults
   (100k retries, 1e-25 floor, per-step Jacobian) are global. Add
   `StiffRadiative` profile carrying them; keep `Default` conservative.
7. **Flatten `StepperOptions.solver_mode + rk_method`** into one
   `StepperSolver` enum (~30 min). Today's split allows silently-invalid
   combos like `Bdf + Tsit45`.
8. **Tsit45 mass-matrix gating** (~30 min). Reject at `build_stepper`
   time with a clear error instead of failing inside diffsol.
9. **Hairer-Wanner auto-h0** (~1 day). Currently `problem.h0` is
   span-relative (`span/5_000_000`) and silently clamped by BDF/SDIRK
   anyway; only useful once Tsit45 works on DAEs.

### Outstanding tasks (lunco-modelica)

10. **Honor `experiment(Solver=, Tolerance=, Interval=)` annotations**
    at FastRun dispatch time. Half-wired today.
11. **Stiffness diagnostics in the Experiments panel**: on failure,
    show `fail_t` as % of horizon + suggest next solver/tolerance.
    Special case: if `fail_t` is bit-identical to 12+ sig figs across
    two runs with different tolerance, surface the message "This
    looks like an IC-solve degeneracy, not a solver tuning issue —
    see the known-failing models table above."
