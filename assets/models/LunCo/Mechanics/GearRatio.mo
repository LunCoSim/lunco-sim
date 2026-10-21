within LunCo.Mechanics;
// Rigid, grounded transmission with imposed mesh efficiency in either power
// direction. Positive terminal power enters the gearbox; the sum leaves as heat.
// This reduced model has no backlash, slip, static friction, or stored energy.
// The torque rating is an operating-envelope check, not a slipping clutch.
model GearRatio
  extends LunCo.Icons.Mechanics;
  parameter Real ratio = 1200.0 "Signed fast/slow shaft speed ratio, nonzero";
  input Real eta(unit="1") = 0.85 "Mesh efficiency from an authored speed/temperature law, 0 < eta <= 1";
  parameter Real max_output_torque(unit="N.m") = 400.0 "Output-shaft torque rating";

  Flange flange_a "Fast side";
  Flange flange_b "Slow side";
  output Real speed_a(unit="rad/s") "Fast-side shaft speed";
  output Real power_a_w(unit="W") "Mechanical power entering flange a";
  output Real power_b_w(unit="W") "Mechanical power entering flange b";
  output Real heat(unit="W") "Mesh loss available to a thermal HeatLoad";
equation
  assert(abs(ratio) > 0.0, "GearRatio requires a nonzero ratio");
  assert(eta > 0.0 and eta <= 1.0, "GearRatio requires 0 < eta <= 1");
  assert(max_output_torque > 0.0, "GearRatio requires a positive torque rating");
  flange_a.phi = ratio * flange_b.phi;
  speed_a = der(flange_a.phi);
  power_a_w = flange_a.tau * speed_a;
  // At rest the forward relation selects a stateless torque branch; no power
  // flows. A finite holding/stiction envelope requires a friction model.
  flange_b.tau = -ratio * flange_a.tau *
    (if noEvent(power_a_w >= 0.0) then eta else 1.0 / eta);
  power_b_w = flange_b.tau * der(flange_b.phi);
  heat = power_a_w + power_b_w;
  assert(abs(flange_b.tau) <= max_output_torque,
    "GearRatio output torque exceeds its rating; model overload or slip explicitly");
end GearRatio;
