// Repro for rumoca bug: `MissingStateEquation("damper.w_rel")`.
//
// The MSL `PartialCompliantWithRelativeStates` (used by every
// Rotational.Components.Spring / Damper / SpringDamper) declares both
// `phi_rel` and `w_rel` as preferred states with equations:
//
//     w_rel = der(phi_rel);
//     a_rel = der(w_rel);    // a_rel typically unused
//
// When `a_rel` has no downstream consumer, rumoca's post-structure
// trivial-elim pass removes that equation. State `w_rel` is left in
// `dae.states` with zero equations defining `der(w_rel)`, so
// `reorder_equations_for_solver` raises `MissingStateEquation`.
//
// Every `Modelica.Mechanics.Rotational.*` example (First,
// PID_Controller, …) hits this.
//
// * On `origin/main` of rumoca — Compile fails with
//   `MissingStateEquation("damper.w_rel")`.
// * On branch `fix/demote-orphan-states-after-post-structure-elim`
//   — Compile succeeds and the stepper builds.
//
// To run: File → Open in the workbench, Compile. Watch Diagnostics.
model SpringDamperRepro "Drive train exercising the compliant-states DAE path"
  parameter Real amplitude = 10 "Driving torque amplitude";
  parameter Real f = 5 "Driving torque frequency";
  parameter Real Jmotor = 0.1 "Motor inertia";
  parameter Real Jload = 2 "Load inertia";
  parameter Real damping = 10 "Bearing damping (triggers damper.w_rel state)";

  Modelica.Mechanics.Rotational.Components.Fixed fixed;
  Modelica.Mechanics.Rotational.Sources.Torque torque(useSupport = true);
  Modelica.Mechanics.Rotational.Components.Inertia inertia1(J = Jmotor);
  Modelica.Mechanics.Rotational.Components.Inertia inertia2(
    J = Jload,
    phi(fixed = true, start = 0),
    w(fixed = true, start = 0));
  Modelica.Mechanics.Rotational.Components.Spring spring(
    c = 1.0e4,
    phi_rel(fixed = true));
  Modelica.Mechanics.Rotational.Components.Damper damper(d = damping);
  Modelica.Blocks.Sources.Sine sine(amplitude = amplitude, f = f);
equation
  connect(sine.y, torque.tau);
  connect(torque.support, fixed.flange);
  connect(torque.flange, inertia1.flange_a);
  connect(inertia1.flange_b, spring.flange_a);
  connect(spring.flange_b, inertia2.flange_a);
  connect(damper.flange_a, inertia1.flange_b);
  connect(damper.flange_b, fixed.flange);
  annotation(experiment(StopTime = 1.0, Interval = 0.001));
end SpringDamperRepro;
