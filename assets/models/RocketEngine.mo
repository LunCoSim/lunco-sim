// tagline: Liquid rocket — thrust, mass flow, total impulse (equation-only)
model RocketEngine
  "Simplified liquid rocket engine — thrust from propellant mass flow and exhaust velocity"

  // ── Design parameters ──
  parameter Real m_dot_max = 15.0 "Max propellant mass flow rate (kg/s)";
  parameter Real v_e = 3100.0 "Effective exhaust velocity (m/s) — LOX/LH2-class";
  parameter Real p_chamber_max = 5e6 "Rated chamber pressure (Pa)";
  parameter Real m_prop_initial = 8000.0 "Initial propellant mass (kg)";
  parameter Real throttle_min = 0.01 "Throttle deadband — at or below this the engine is off";
  parameter Real m_prop_band = 1.0 "Burnout taper width (kg) — flow ramps to 0 over the last kg";

  // ── Runtime inputs ──
  input Real throttle = 0.0 "Throttle command, 0..1 (default off — script/user controls it)";

  // ── State ──
  Real m_prop(start=m_prop_initial) "Propellant remaining (kg)";
  Real impulse(start=0) "Total impulse delivered (N·s)";

  // ── Observables ──
  // rumoca is BRANCH-FREE: an `if` in an equation section is not compiled — the
  // guarded algebraic is reconstructed as literal 0, so every dependent observable
  // silently reads zero. A Boolean `burning` intermediate does not help either: the
  // algebraic-elimination reconstructor only evaluates continuous substitutions.
  // So the "engine is burning" test is carried by CONTINUOUS 0..1 gate variables
  // built from `max`/`min` — exactly 0 when off, exactly 1 in the operating region,
  // with a narrow ramp between that also spares the solver a step discontinuity.
  Real prop_gate "Propellant-remaining gate, 0..1 (0 once the tank is dry)";
  Real thr_gate "Throttle deadband gate, 0..1 (1 for throttle >= 2*throttle_min)";
  Real m_dot "Instantaneous mass flow (kg/s)";
  Real thrust "Thrust (N)";
  Real p_chamber "Chamber pressure (Pa)";
  Real isp "Specific impulse (s)";

equation
  prop_gate = min(max(m_prop / m_prop_band, 0.0), 1.0);
  thr_gate = min(max((throttle - throttle_min) / throttle_min, 0.0), 1.0);

  m_dot = m_dot_max * throttle * prop_gate * thr_gate;
  thrust = m_dot * v_e;
  p_chamber = p_chamber_max * throttle * prop_gate * thr_gate;
  isp = v_e / 9.80665;

  der(m_prop) = -m_dot;
  der(impulse) = thrust;
end RocketEngine;
