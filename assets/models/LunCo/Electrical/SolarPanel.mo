within LunCo.Electrical;
// A photovoltaic source on the bus. Its output is `area × efficiency × irradiance` at
// normal incidence, derated by the live mount-frame Sun vector and the panel normal,
// then clamped to the lit hemisphere. It pushes that power onto the bus as current at
// the bus voltage — `p.i` is negative because current LEAVES the panel into the node.
model SolarPanel
  extends LunCo.Icons.SolarPanel;
  parameter Real area = 6.0 "Collecting area, m2";
  parameter Real efficiency = 0.30 "Irradiance-to-electrical conversion, 0..1";
  parameter Real peak_power_rating_w = Modelica.Constants.inf
    "Maximum rated electrical output, W; infinity means no nameplate limit";
  // Module voltage at the maximum-power point. A PV module's photocurrent is set
  // by the light; this is the operating voltage that current is rated at, and it
  // is a property of the module, not of the bus it is bolted to.
  parameter Real v_mp = 48.0 "Module voltage at maximum power, V";

  input Real irradiance "Incident irradiance, W/m2";
  input Real target_mount_x "Target direction in the panel frame, +X right";
  input Real target_mount_y "Target direction in the panel frame, +Y up";
  input Real target_mount_z "Target direction in the panel frame, -Z forward";
  input Real panel_normal_x "Panel illuminated-face normal in the electrical assembly frame";
  input Real panel_normal_y "Panel illuminated-face normal in the electrical assembly frame";
  input Real panel_normal_z "Panel illuminated-face normal in the electrical assembly frame";

  Pin p annotation(Placement(transformation(extent={{-110,-10},{-90,10}})));
  output Real power_out(unit="W") "Electrical power delivered to the bus after bus loading";
  output Real available_power_w(unit="W") "Electrical power available from incident sunlight before bus loading";
  output Real cos_incidence "Live clamped cosine of solar incidence, 0..1";
  output Real terminal_voltage_v(unit="V") "Electrical bus voltage at the solar-panel terminals";
  output Real generated_current_a(unit="A") "Current delivered from the panel to the electrical bus";

  Real panel_normal_norm "Magnitude of the authored panel normal";
  Real alignment "Signed alignment of the unit target direction and panel normal";
equation
  panel_normal_norm = sqrt(max(1.0e-12,
    panel_normal_x^2 + panel_normal_y^2 + panel_normal_z^2));
  alignment = (
    target_mount_x * panel_normal_x
    + target_mount_y * panel_normal_y
    + target_mount_z * panel_normal_z
  ) / panel_normal_norm;
  cos_incidence = min(max(alignment, 0.0), 1.0);

  // A PV MODULE IS A CURRENT SOURCE. Photocurrent is proportional to the light
  // falling on it and is very nearly independent of terminal voltage across the
  // whole operating range — that is the defining characteristic of a
  // photovoltaic device, and it is why panels are rated by short-circuit current.
  // `p.i` is negative because current LEAVES the panel into the node.
  available_power_w = min(
    max(area * efficiency * irradiance * cos_incidence, 0.0),
    max(peak_power_rating_w, 0.0));
  p.i = -available_power_w / v_mp;

  // Delivered power FOLLOWS from the current and the bus voltage it actually
  // meets, so droop still shows up here — it is an output, not a driver.
  power_out = -p.i * p.v;
  terminal_voltage_v = p.v;
  generated_current_a = -p.i;

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
