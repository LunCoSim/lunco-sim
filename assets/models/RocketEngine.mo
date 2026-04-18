model RocketEngine
  "Simplified liquid rocket engine — thrust from propellant mass flow and exhaust velocity"

  // ── Design parameters ──
  parameter Real m_dot_max = 120.0 "Max propellant mass flow rate (kg/s)";
  parameter Real v_e = 2900.0 "Effective exhaust velocity (m/s) — LOX/kerosene-class";
  parameter Real p_chamber_max = 10e6 "Rated chamber pressure (Pa)";
  parameter Real m_prop_initial = 4000.0 "Initial propellant mass (kg)";

  // ── Runtime inputs ──
  input Real throttle = 1.0 "Throttle command, 0..1";

  // ── State ──
  Real m_prop(start=m_prop_initial) "Propellant remaining (kg)";
  Real impulse(start=0) "Total impulse delivered (N·s)";

  // ── Observables ──
  // Inline the "engine is burning" test everywhere instead of a Boolean
  // intermediate: rumoca's algebraic-elimination reconstructor only
  // evaluates continuous substitutions, so a separate Boolean `burning`
  // would read as 0 at runtime and zero out all dependent observables.
  Real m_dot "Instantaneous mass flow (kg/s)";
  Real thrust "Thrust (N)";
  Real p_chamber "Chamber pressure (Pa)";
  Real isp "Specific impulse (s)";

equation
  m_dot = if m_prop > 0.0 and throttle > 0.01 then m_dot_max * throttle else 0.0;
  thrust = m_dot * v_e;
  p_chamber = if m_prop > 0.0 and throttle > 0.01 then p_chamber_max * throttle else 0.0;
  isp = v_e / 9.80665;

  der(m_prop) = -m_dot;
  der(impulse) = thrust;
end RocketEngine;
