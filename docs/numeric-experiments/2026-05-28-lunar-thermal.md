# 2026-05-28 — Lunar rover thermal model integration

## TL;DR

The `LunarRover.RoverThermalSystem` model in `RoverThermalModular.mo`
wouldn't integrate past `t = 2.5e-7 s` on any solver (BDF, ESDIRK34,
TR-BDF2), even though OMEdit handles it fine. After diagnosis, the
working configuration is:

- `solver = "tr_bdf2"`
- `tolerance = 1e-3` (not the conventional 1e-6)
- `dt = 3600` (output interval, 1 hour)
- New permissive rumoca defaults (see fix section)

With this, integration scales linearly: **1 lunar day → 10s wall**,
**24 lunar days (~2 earth years) → 25s wall**.

## Problem

Reference model: `crates/lunco-modelica/../../RoverThermalModular.mo`,
class `LunarRover.RoverThermalSystem`. Thermal model of a lunar rover:

- Body lump (50 kJ/K, starts at 293 K)
- Louver radiator with `tanh` emissivity hysteresis (eps 0.1↔0.85
  around T_louver_mid=300 K, T_width=5 K)
- Bimetallic-style heater with `tanh` on/off hysteresis (T_on=263 K,
  T_off=273 K, transition width 1.0 K)
- MLI radiative loss to lunar surface
- Leg conduction to ground
- Solar input modulated by `tanh(10*sin(2π·phase))` → near-discontinuous
  day/night terminator
- Surface temperature swings between 100 K and 390 K over lunar day
- Lunar day = 2,551,392 s ≈ 29.53 earth days

Target: integrate over at least one lunar day, ideally multiple months.

**Failure mode**:
```
SolverError("Step failed: ODE solver error: Step size is too small at time = 0.0000002514555228787582")
```
On every solver. Bit-identical `fail_t = 2.514555228787582e-7` to 16 sig
digits across BDF runs. ESDIRK34: `2.937e-7`. The 16-digit reproducibility
is the giveaway — it's not a step-size choice, it's the solver
deterministically halving its way to the min-step floor.

## Symptoms

| Solver | tol | fail_t | steps | nl_iters | nl_fails | linear_setups |
|---|---|---|---|---|---|---|
| BDF | 1e-6 | 2.51e-7 | 768 | 2508 | 11 | 96 |
| ESDIRK34 | 1e-6 | 2.94e-7 | 941 | 12458 | 15 | 102 |
| TR-BDF2 | 1e-6 | 2.51e-7 | 3249 | 19074 | 11 | 277 |
| Tsit45 | – | (init fail) | – | – | – | – |

Tsit45 errored at construction with `"Mass matrix not supported for this
solver"` — explicit RK can't accept the mass matrix rumoca emits for
scalarized DAEs.

Newton failure rate **looked fine** on every implicit solver (0.06%–0.4%).
The error wasn't "the solver can't solve the Newton system" — it was "the
solver keeps halving step size until it falls below diffsol's `min_step`
floor of 1e-16".

## Investigation

### Hypothesis 1: `h0` is too large

Falsified by an h0 sweep. With manual h0 ∈ {default ≈ 5103s, 1e-3, 1e-2,
1e-1, 1.0}, every BDF run failed at exactly `t = 2.514555228787582e-7`
with identical step counts, nl_iters, nl_fails. Same story for ESDIRK34
at `2.937e-7`. **h0 is silently clamped** by diffsol's BDF/SDIRK
internals; the value we pass is ignored once the first error-test fails.

Why: BDF and SDIRK families compute their own initial step as part of
warmup, using a local Lipschitz estimate. User `problem.h0` is just a
hint; the solver does what it thinks is right and halves from there. The
`t = 2.5e-7` is the cumulative time after ~30 halvings down to the
`min_step = 1e-16` floor — deterministic and reproducible because the
algorithm is deterministic.

### Hypothesis 2: min-step floor is wrong

Lowered `min_timestep` from 1e-16 to 1e-25 and ran the sweep again.
Result: `fail_t` shifted to `2.512445097077247e-7` (essentially the
same — gained a few halvings of headroom). Step count doubled to 1499,
nl_iters jumped to 11029, nl_fails to 124. Still hits a wall.

### Hypothesis 3: stale finite-difference Jacobian

Forced `update_jacobian_after_steps = 1` and `update_rhs_jacobian_after_steps = 1`.
No change in failure point. Fresh Jacobian doesn't help if the Jacobian
itself is wrong.

### Diagnosis: FD Jacobian on σT⁴ is numerically degenerate

The radiator term `Q_flow = ε·σ·A·(T_a⁴ - T_b⁴)` produces:

- At T = 293 K: σT⁴ = 5.67e-8 × 293⁴ ≈ 419 W/m²
- Finite-difference perturbation: ε = √(2.2e-16) · 293 ≈ 4.4e-6 K
- σ(T+ε)⁴ - σT⁴ ≈ 4σT³ · ε ≈ 2.5e-5 W/m²

That's a 2.5e-5 perturbation in a 419 magnitude. The FD Jacobian loses
**~9 significant digits to cancellation**. For the consistent-IC solve
at t=0, Newton is iterating on a Jacobian that's effectively random
noise. It "converges" — to whatever direction the rounding error
points. The integrator then starts at a `y₀` that satisfies the residual
*to FD precision* but is far from the true manifold, and the first step
sees a huge local error → halve → halve → halve.

This is exactly the class of problem OMC/DASSL solves with analytical
Jacobians and homotopy initialization. Diffsol+rumoca have neither.

### Hypothesis 4: insufficient retry budget hides the symptom

Bumped `max_nonlinear_solver_failures` 1000 → 100_000 and
`max_error_test_failures` 600 → 100_000. Combined with the 1e-25 floor
and IC-linesearch (next section), this is the actual fix.

### Hypothesis 5: IC solver is converging to a degenerate y₀

Enabled `ic_options.use_linesearch = true` with `max_newton_iterations =
200` and `max_linesearch_iterations = 40`. This lets the consistent-IC
solver back off along the Newton direction when the full step lands at
worse residual — finding a feasible `y₀` even with a noisy Jacobian.

### Hypothesis 6: model needs `start=` annotations

Tested. Adding `start=`/`nominal=` to algebraic vars
(`louver_pos`, `eps_eff`, `Q_flow`, `dT`, `heater_on`, `Q_heater`,
`phase`) helps somewhat but is **not essential**. Ablation showed BDF +
new rumoca settings + tol=1e-3 completes 1 lunar day in 12.7s wall
without any model edits.

## Root cause

The FD Jacobian on the radiative `σT⁴` network plus sharp `tanh`
transitions produces a near-singular Jacobian at `t = 0`. Default rumoca
settings (1000 retries, 1e-16 floor, no IC linesearch) trip before the
integrator can escape the singularity.

## Fix

### Rumoca-solver-diffsol settings (in `configure_solver_problem_with_profile`)

```rust
problem.ode_options.max_nonlinear_solver_iterations = 50;      // was 20
problem.ode_options.max_nonlinear_solver_failures = 100_000;   // was 1000
problem.ode_options.max_error_test_failures = 100_000;         // was 600
problem.ode_options.min_timestep = 1e-25;                      // was 1e-16
problem.ode_options.update_jacobian_after_steps = 1;           // was 20
problem.ode_options.update_rhs_jacobian_after_steps = 1;       // was 5
problem.ic_options.use_linesearch = true;                      // was false
problem.ic_options.max_newton_iterations = 200;                // was 50
problem.ic_options.max_linesearch_iterations = 40;             // was 10
```

Plus new public knobs in `StepperOptions`:
- `solver_mode: SimSolverMode` (Auto/Bdf/RkLike)
- `rk_method: RkMethod` (Esdirk34/TrBdf2/Tsit45)
- `initial_step: Option<f64>`

`build_stepper` now branches on `solver_mode + rk_method` instead of
hardcoding BDF.

### Run parameters (FastRunActiveModel API)

```json
{
  "doc": <id>,
  "class": "LunarRover.RoverThermalSystem",
  "t_end": 2551392,
  "dt": 3600,
  "tolerance": 1e-3,
  "solver": "tr_bdf2"
}
```

**`tol = 1e-3` is the practical sweet spot.** Tighter tolerances (1e-4,
1e-5, 1e-6) force enough step rejections at the louver-crossing /
heater-toggle events to exhaust even the 100k retry budget.

**TR-BDF2 is required for multi-day horizons.** BDF works for 1 lunar day
but bails at the second sunrise's louver crossing (t ≈ 5.59M s ≈ 2.19
lunar days). TR-BDF2's event-handling is robust to that.

## Validation

Full lunar-day + multi-month sweep with TR-BDF2, tol=1e-3, dt=3600s:

| Horizon | Wall time | Samples | Notes |
|---|---|---|---|
| 1 lunar day | 10 s | 709 | baseline |
| 2 lunar days | 10 s | 1418 | first louver crossing on 2nd sunrise — OK |
| 3 lunar days | 15 s | 2127 | OK |
| 6 lunar days | 15 s | 4253 | OK |
| 12 lunar days (~1 earth yr) | 20 s | 8505 | OK |
| 24 lunar days (~2 earth yrs) | 25 s | 17010 | OK |
| 36 lunar days at dt=10800 | – | – | fails at t≈486000 s (dt-spacing artifact?) |

BDF at tol=1e-3 completes 1 lunar day cleanly but fails on the second
sunrise. Use TR-BDF2 for anything > 1 lunar day.

Without `start=` annotations (ablation): BDF tol=1e-3 still works at 1
lunar day. With them, marginally fewer nl_fails. Not required.

## What does *not* work / does *not* help

- Manual `h0` override: silently clamped by BDF/SDIRK internals
- Lowering `min_timestep` further than 1e-25: machine-precision wall
- Per-step Jacobian update alone (without retry-budget bump)
- ESDIRK34 at tol ≤ 1e-3: chokes harder than BDF; works only at 1e-2
- Tsit45: incompatible (mass matrix rejected by explicit RK)
- Tolerance ≤ 1e-4: every solver hits the retry budget mid-simulation
  at the louver/heater event walls

## TBDs / future work

### Rumoca

1. **Symbolic Jacobian via rumoca AST + cranelift**. The real fix for
   the FD-Jacobian degeneracy class of problems. Rumoca already has the
   symbolic AST and a cranelift JIT for the residual; adding a `∂/∂y`
   pass + a second JIT'd function would close ~80% of the remaining
   gap to OMC/DASSL on stiff radiative models. **Weeks of work,
   highest leverage.**

2. **Per-state `atol` vector honoring Modelica `nominal=`**.
   `SimVariableMeta.nominal` is already parsed but ignored at solver
   setup. Let `louver_pos ∈ [0,1]` and `T ∈ [100, 400]` use
   different absolute tolerances. **~1 day, high leverage.**

3. **Tiered solver profiles**. Today's aggressive defaults (100k
   retries, 1e-25 floor, per-step Jacobian) are global and will slow
   non-stiff workloads. Tier via existing `SolverStartupProfile`:
   Default stays conservative; add a `StiffRadiative` profile that
   carries these settings; pick automatically based on AST analysis
   (presence of `^4` on temperature-like states + radiation patterns).

4. **Flatten the public solver enum**.
   `StepperOptions.solver_mode + rk_method` is a two-knob trap with
   silently-invalid combinations (`Bdf + Tsit45` is ignored, not
   rejected). Collapse to a single flat `StepperSolver` enum at the
   public API boundary; keep `SimSolverMode + RkMethod` private as
   implementation detail. **~30 min.**

5. **Tsit45 mass-matrix gating**. Either reject Tsit45 at
   `build_stepper` time with a clear error when the DAE has M ≠ I, or
   detect scalarized-ODE form and allow it only there. Currently fails
   inside diffsol's constructor with a generic error.

6. **Hairer-Wanner auto-h0**. Replace the `span / 5_000_000` magic
   number with two extra `eval_compiled_runtime_residual` calls:
   `h0 = (rtol / max(||f||, ||f'||))^(1/(p+1))`. Auto-tunes per
   problem. **~1 day.** Note: BDF/SDIRK ignore h0 anyway today, so
   this is mostly useful once we have explicit RK working on DAEs and
   the principled-but-currently-dead-letter Tsit45 path.

7. **Homotopy initialization** for the consistent-IC solve. Currently
   relies on `start=` values + plain Newton. Homotopy / continuation
   would unblock models without `start=` annotations even when the
   Jacobian is pathological at t=0. **~½ day.**

8. **Lower diffsol's min-step floor** (or get diffsol to expose it).
   Hardcoded inside diffsol; we work around with `min_timestep = 1e-25`
   but that's at the rumoca→diffsol seam, not the floor itself.
   Upstream PR.

### lunco-modelica

9. **Bind `dt`, `tolerance`, and `solver` defaults from the model's
   `experiment(Tolerance=, StopTime=, Solver=, Interval=)` annotation**.
   Half-done: `ModelDefaults` carries them, but `experiment(Solver=)`
   parsing isn't wired up. Would mean models can ship with their
   working solver choice baked in.

10. **Surface stiffness diagnostics in the Experiments panel**. When a
    run fails, show: solver kind, `fail_t` as % of horizon, nl_iters,
    nl_fails, suggestion ("try `tr_bdf2`" if BDF failed at a
    >1-lunar-day repeat point, "try `tolerance=1e-3`" if hit retry
    budget, etc.).

## References

- Rumoca commit: `feat(solver-diffsol): expose RK tableau selection +
  permissive defaults for stiff radiative DAEs`
  (`luncosim-workspace/rumoca` main branch)
- Memory note: `~/.claude/.../memory/project_lunar_thermal_solver_settings.md`
- Diffsol upstream: `crates.io diffsol 0.10.4`
  (`/home/rod/.cargo/registry/src/.../diffsol-0.10.4/src/ode_solver/`)
- Hairer & Wanner, *Solving Ordinary Differential Equations II:
  Stiff and Differential-Algebraic Problems*, §IV.5 (initial-step
  estimation), §IV.8 (DASSL-family stiff DAE solvers).
