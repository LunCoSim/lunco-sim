within LunCo.Electrical;
// A photovoltaic source on the bus. Its output is `area × efficiency × irradiance` at
// normal incidence, derated by the cosine of the sun angle and clamped to the lit
// hemisphere. It pushes that power onto the bus as current at the bus voltage — `p.i` is
// negative because current LEAVES the panel into the node.
model SolarPanel
  parameter Real area = 6.0 "Collecting area, m2";
  parameter Real efficiency = 0.30 "Irradiance-to-electrical conversion, 0..1";
  // Module voltage at the maximum-power point. A PV module's photocurrent is set
  // by the light; this is the operating voltage that current is rated at, and it
  // is a property of the module, not of the bus it is bolted to.
  parameter Real v_mp = 48.0 "Module voltage at maximum power, V";

  input Real irradiance "Incident irradiance, W/m2";
  input Real cos_incidence "Cosine of the sun incidence angle, 0..1";

  Pin p;
  output Real power_out "Electrical power delivered to the bus, W";
equation
  // A PV MODULE IS A CURRENT SOURCE. Photocurrent is proportional to the light
  // falling on it and is very nearly independent of terminal voltage across the
  // whole operating range — that is the defining characteristic of a
  // photovoltaic device, and it is why panels are rated by short-circuit current.
  // `p.i` is negative because current LEAVES the panel into the node.
  p.i = -(area * efficiency * irradiance * max(0.0, cos_incidence)) / v_mp;

  // Delivered power FOLLOWS from the current and the bus voltage it actually
  // meets, so droop still shows up here — it is an output, not a driver.
  power_out = -p.i * p.v;

  // WHY THE DIRECTION MATTERS. This used to read `p.i = -power_out / p.v`, i.e.
  // a constant-POWER source. Every device on the node metered itself that way,
  // and the battery closes the node with `p.v = f(soc) + p.i*R`, so `sum(i) = 0`
  // became a NONLINEAR ALGEBRAIC LOOP in `p.v`. The live stepper refreshes
  // algebraic rows one at a time (pair a row with a variable, secant-solve it)
  // and cannot solve a simultaneous system: it failed with `algebraic refresh
  // row 2 cannot be solved for 'Battery.p.i': the residual does not depend on
  // it`, the island published no ports, and every solar rover was silently dead.
  //
  // Writing the physics in its natural causal direction removes the loop rather
  // than approximating around it: current out of light, power out of current.
end SolarPanel;
