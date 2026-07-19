model LegStrut "Landing-leg shock strut: contact-gated spring-damper. One instance per leg; outputs the stroke the visual actuator applies."
  // The airframe wires these from its own sensors (see descent_lander.usda):
  //   Altimeter.range ──► altitude      body velocity_y ──► descent_rate
  input Real altitude = 100.0 "Altimeter range (m); the pads touch at ~contact_alt";
  input Real descent_rate = 0.0 "Body vertical speed (m/s), negative down";

  // A parameter is an input with a constant instead of a connection — the
  // same convention as DescentGuidance.mo, so a leg can be re-tuned from USD.
  input Real contact_alt = 1.8 "Altimeter range (m) when the pads first touch";
  input Real gate_width = 0.4 "Contact-gate softness (m) — the blend band over which load arrives";
  input Real k = 4000.0 "Spring rate (N/m); static settle = m_eff*g/k = 0.20 m";
  input Real c = 2200.0 "Damper (N s/m); zeta ~ 0.78 against k and m_eff — one visible overshoot, no ringing";
  input Real m_eff = 500.0 "Vehicle mass share carried by this leg (kg) — a quarter of the 2000 kg airframe";
  input Real g = 1.62 "Lunar gravity (m/s^2)";
  input Real kv_impact = 900.0 "Extra axial load per m/s of arrival sink (N s/m) — what makes a hard landing flex deeper";
  input Real stroke_max = 0.8 "Mechanical stop (m); the piston sleeve is 2.6 m, the strut never bottoms out visually";

  Real x(start = 0.0) "Strut stroke state (m)";
  Real v(start = 0.0) "Stroke rate (m/s)";
  Real contact "0..1 pad-contact gate";
  Real load "Axial load on the strut (N)";

  output Real stroke "Clamped visible compression (m) for scenarios/leg_spring.rhai";
equation
  // Branch-free (rumoca-safe) throughout, like DescentGuidance.mo.
  // Contact fades in over `gate_width` of altimeter range: at 2 m/s of sink
  // that is ~0.2 s of blend, during which `descent_rate` is still negative —
  // so the impact term samples the true arrival speed, then dies with it.
  contact = min(1.0, max(0.0, (contact_alt - altitude) / gate_width));
  load = contact * (m_eff * g + kv_impact * max(0.0, -descent_rate));
  der(x) = v;
  // Explicit solved form — every der() rumoca compiles in this asset tree
  // (Lander.mo) is spelled `der(state) = expr`, so this one is too.
  der(v) = (load - k * x - c * v) / m_eff;
  stroke = min(stroke_max, max(0.0, x));
end LegStrut;
