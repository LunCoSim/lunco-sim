model LegStrut "Landing-leg shock strut: a spring-damper driven by the pad's measured contact force. One instance per leg; outputs the stroke the visual actuator applies."
  // The airframe wires this from the PAD'S OWN COLLIDER (see descent_lander.usda):
  //   PadPX.contact_force ──► pad_force
  //
  // From PHYSICS, not from a sensor. A leg compresses because its pad is being
  // pushed on, and avian already knows that force — the strut is a physical part,
  // so it reads the physical fact. Sensors are the other layer: they model
  // INSTRUMENTS (mount offset, range limits, failure) and exist for flight
  // software. `DescentGuidance.mo` reads the altimeter because a computer only
  // knows what its instruments report; a spring does not consult an altimeter.
  //
  // This model used to be gated on `Altimeter.range` against a `contact_alt`
  // constant, and that was the bug. The altimeter's datum sits at the nozzle
  // bottom, 3.3 m above the pads, so `contact_alt` existed purely to translate
  // between two prims' positions — a geometric fact hand-copied onto four legs,
  // free to drift from the geometry it described. It drifted: authored at 5.6 m
  // (the vessel-origin altitude the HUD shows) against a port carrying sensor
  // range, contact opened with the pads still 3.9 m in the air and the legs lit
  // before the bump. Measured contact force cannot be early — contact IS the
  // collision — and there is no constant left to disagree with anything.
  input Real pad_force = 0.0 "Contact normal force on this leg's pad (N), from the collider";

  // A parameter is an input with a constant instead of a connection — the
  // same convention as DescentGuidance.mo, so a leg can be re-tuned from USD.
  input Real k = 4000.0 "Spring rate (N/m); steady stroke = pad_force / k";
  input Real c = 2200.0 "Damper (N s/m); size against 2*sqrt(k*m_strut) — ~0.7 of it gives one visible overshoot, no ringing";
  // The STRUT'S OWN moving mass — the piston and damper assembly, not the
  // vehicle. This was `m_eff = 500`, a quarter of the 2000 kg airframe, and that
  // was double-counting: avian is already simulating the vehicle's mass and its
  // deceleration against the ground. Putting it here too made a 500 kg mass on a
  // 1400 N/m spring, natural period ~3.7 s, so the strut took about a second to
  // reach its settle and the colour crawled up behind it — which is exactly why
  // it read as an ANIMATION rather than an impact.
  //
  // With the strut's real mass the response is ~0.1 s: the deflection tracks the
  // measured contact force almost instantly, so what is drawn IS the force. The
  // steady stroke is unchanged (pad_force / k), because that never depended on
  // mass — only the speed of getting there did.
  input Real m_strut = 12.0 "Strut's own moving mass (kg) — piston + damper, NOT the vehicle";
  input Real stroke_max = 0.8 "Mechanical stop (m); the piston sleeve is 2.6 m, the strut never bottoms out visually";

  Real x(start = 0.0) "Strut stroke state (m)";
  Real v(start = 0.0) "Stroke rate (m/s)";

  output Real stroke "Clamped visible compression (m) for scenarios/leg_spring.rhai";
  output Real load "Axial load on the strut (N)";

  // The strut's own opinion of how hard it is working, 0..1. The colour ramp
  // the actuator paints is a function of THIS — the normalisation lives with
  // the spring that knows its own rating, not in the script that draws it.
  // Change `load_rated` here and the whole fleet's glow re-scales; no script
  // and no material edit.
  parameter Real load_rated = 1500.0 "Load (N) the strut is rated for — full-scale for the glow";
  output Real load_frac "load / load_rated, clamped to 0..1";
equation
  // Branch-free (rumoca-safe) throughout, like DescentGuidance.mo.
  //
  // The strut is driven by the measured contact force and NOTHING ELSE. There is
  // no gate, no blend band, and no arrival term: `pad_force` is exactly zero
  // until the pad touches, and a fast touchdown already arrives as a larger
  // normal impulse than a slow settle. The old `kv_impact * descent_rate` term
  // existed to reconstruct that impact from vehicle speed because the proximity
  // gate could not see the collision; against real contact force it would
  // double-count the very spike physics is handing over.
  der(x) = v;
  // Explicit solved form — every der() rumoca compiles in this asset tree
  // (Lander.mo) is spelled `der(state) = expr`, so this one is too.
  der(v) = (pad_force - k * x - c * v) / m_strut;
  stroke = min(stroke_max, max(0.0, x));

  // `load` is the force IN THE STRUT — the spring-damper's own reaction,
  // k*x + c*v — not the driving term above.
  //
  // This distinction is the difference between honest and decorative. Publishing
  // `drive` meant the leg reported the force being applied TO it, which is
  // nonzero the instant the pad kisses the ground; a spring cannot push until it
  // is compressed. With the reaction force, `load` is exactly zero until x > 0,
  // rises as the strut takes the vehicle, peaks at maximum compression, and
  // settles to whatever the ground is holding up — so the colour is a consequence of the
  // landing rather than a prediction of it.
  load = k * stroke + c * v;
  load_frac = min(1.0, max(0.0, load / load_rated));
end LegStrut;
