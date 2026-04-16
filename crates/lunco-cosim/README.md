# lunco-cosim

Co-simulation orchestration for LunCoSim. Connects multiple simulation models
(Modelica, FMU, GMAT, Avian) in a single Bevy world via explicit wires that
route named outputs to named inputs, following the FMI/SSP pattern.

## Architecture at a glance

Every simulation engine is treated as a model with named inputs and outputs:

| Model                       | Inputs                     | Outputs                              |
| --------------------------- | -------------------------- | ------------------------------------ |
| **AvianSim**                | `force_y`, `force_x`, ...  | `position_y`, `velocity_y`, `height` |
| **SimComponent** (Modelica) | `height`, `velocity`, ...  | `netForce`, `volume`, ...            |
| **SimComponent** (FMU)      | `current_in`, ...          | `soc`, `voltage`, ...                |

A [`SimConnection`] connects any output to any input. The co-sim master runs in
`FixedUpdate` in two ordered sets:

1. **`Propagate`** — `propagate_connections` reads every source output and writes
   the target input (accumulating with `+=` so multiple wires can sum into one
   input).
2. **`ApplyForces`** — `apply_sim_forces` integrates `netForce → LinearVelocity`
   for kinematic balloons. Avian's own `integrate_positions` advances `Position`
   from `LinearVelocity` later in `FixedPostUpdate`.

`read_avian_outputs` runs after `PhysicsSystems::Writeback` in
`FixedPostUpdate`, populating `AvianSim.outputs` from `Position` +
`LinearVelocity` so the next frame's wires see fresh values.

## Modelica model convention: declare everything you want to observe as `input` or `output`

**The rule:** in any Modelica model driven by `lunco-cosim`, every variable
that needs to be read by the co-simulation wires — or written into the model
from outside — must have explicit `input` or `output` causality.

```modelica
model Balloon
  parameter Real mass = 4.5;

  // Wires feed these in from Avian each step
  input Real height = 0;
  input Real velocity = 0;

  // State — rumoca keeps this in the solver regardless
  Real volume(start = 4.0);

  // ALL OBSERVABLE DERIVED VALUES MUST BE `output`
  output Real netForce;
  output Real buoyancy;
  output Real weight;
  output Real drag;
  output Real temperature;
  output Real airDensity;
equation
  // ...
end Balloon;
```

### Why

Rumoca's DAE preparation pipeline aggressively substitutes algebraic variables
into the state equation. If you declare an algebraic as a bare `Real netForce`,
rumoca eliminates it during index reduction — the variable literally stops
existing in the solver. After compile:

- `stepper.variable_names()` returns `["volume"]` — `netForce` is gone.
- `stepper.get("netForce")` returns `None` — no name-based recovery either.

So `SimComponent.outputs["netForce"]` never gets populated, the
`netForce → force_y` wire has nothing to read, no force reaches Avian, and the
balloon sits still.

Declaring a variable with `output` causality tells rumoca it's part of the
model's public interface and must be preserved in the solver index. After that
single keyword change, `stepper.get("netForce")` returns a real value and the
whole cosim chain lights up.

This is a [known, reproducible rumoca limitation](#upstream-rumoca-workaround)
— the convention above is both the fix *and* good FMI/SSP hygiene: a model's
inputs and outputs are its public interface; everything else is private
implementation.

### Inputs: same story

Inputs without explicit `input` causality get inlined as constants. Always
declare them:

```modelica
input Real height = 0;   // runtime-settable, default 0
input Real velocity = 0;
```

The default value (`= 0`) is stripped by `lunco-modelica` at compile time
(`strip_input_defaults`) so the variable becomes a true runtime slot
settable via `stepper.set_input("height", ...)`. Without the `input` keyword
at all, the variable would vanish like the algebraics did.

### Checklist when adding a new Modelica model to lunco-cosim

1. Mark every wire-destination as `input Real name = <default>;`.
2. Mark every wire-source as `output Real name;`.
3. States can stay bare (`Real x(start = ...);`) — rumoca always keeps them.
4. Parameters stay as `parameter Real foo = 1.0;`.
5. Run [`balloon_stepper_test.rs`](../lunco-modelica/tests/balloon_stepper_test.rs)
   pattern against your model: compile with rumoca, assert
   `stepper.get("<your_variable>").is_some()` for every variable you plan
   to wire. If it fails, you forgot an `output` somewhere.

## Tests

- **`tests/balloon_cosim_test.rs`** — full-stack regression test. Boots a
  headless Bevy app with `CoSimPlugin` + `ModelicaCorePlugin`, spawns a balloon
  entity, lets the Modelica worker compile `balloon.mo`, runs 200+ frames, and
  asserts that:
  1. `SimComponent.outputs["netForce"]` is positive (rumoca returned it),
  2. `SimComponent.inputs["force_y"]` is non-zero (wire propagation works),
  3. `LinearVelocity.y` is non-zero (integration works).

  If this test breaks after a rumoca upgrade or a `balloon.mo` edit, one of
  the three links in the chain regressed — the assertion tells you which.

- **`tests/balloon_e2e_test.rs`** — unit-level wire propagation and force
  application tests with mocked `SimComponent.outputs`. Fast; no Modelica
  worker involved.

- **`lunco-modelica/tests/balloon_stepper_test.rs`** — isolates rumoca itself.
  Compiles `balloon.mo` directly and asserts that `stepper.get("netForce")`
  returns `Some`. This is the regression test for the "algebraics eliminated"
  bug — if it fails, the `output` workaround has stopped working and we need
  to revisit the [upstream fix](#upstream-rumoca-workaround).

Run with:

```bash
cargo test -p lunco-cosim
cargo test -p lunco-modelica --test balloon_stepper_test
```

`lunco-cosim`'s dep graph is small, so these tests recompile in a few seconds
on an incremental build — unlike `lunco-client` tests, which pull in the full
Bevy renderer.

## Upstream rumoca workaround

The `output`-keyword convention works around a limitation in our
[rumoca fork](https://github.com/LunCoSim/rumoca). Rumoca's
`SimStepper::variable_names()` returns only the post-reduction solver state
(`solver_names` truncated to `n_total = dae.f_x.len()`), and
`SimStepper::get(name)` only knows about indices present in the solver — so
after aggressive substitution, algebraic values are unreachable even by name.

A cleaner upstream fix would be one of:

- **Option A:** Don't substitute variables declared with `output` causality in
  `prepare_dae`. Keep them in `dae.f_x` as trivial equations. (Already
  effectively our current behavior — but we're relying on it rather than it
  being documented.)
- **Option B:** Add a `SimStepper::evaluate_algebraic(name) -> Option<f64>`
  API that re-evaluates an eliminated algebraic expression using the current
  state + inputs + parameters. Algebraic expressions still live in
  `dae.algebraics`; we'd just need an expression evaluator over the current
  solver state.
- **Option C:** Keep a side-table of "observable" variables with their
  symbolic expressions, and have `get(name)` fall back to evaluating from that
  table when the name isn't in the solver index.

Until one of these lands, the `output` convention is the supported way to
declare model variables to lunco-cosim.
