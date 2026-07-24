within;
// The solar rover demo's power system: one acausal circuit solved as a DAE.
//
// Battery, SolarPanel and DCMotor from `LunCo.Electrical`, connected on one bus node so
// Kirchhoff balances source against load — SoC falls out of the circuit, nothing sums a
// current. Top-level (`within;`), bound to the scene's `PowerSubsystem` prim via
// `info:sourceAsset` and importing the seated `LunCo` library: components in the
// library, with the assembly projected from each vehicle's USD electrical Scope.
//
// Boundary (where cosim crosses): `sun_azimuth` from the solar bridge, `panel_yaw` from
// the SunTracker, `vehicle_throttle` from the autopilot. The demo wires no wheel speed, so
// the motor runs at a nominal cruise `omega` (a full rover wires real per-wheel `omega`).
model SolarRoverPower "Solar charging vs. motor draw on one battery bus, from LunCo.Electrical."
  import LunCo.Electrical.*;

  // Circuit parameters — VALUES authorable from USD `inputs:` on the prim.
  parameter Real battery_capacity = 2.0 "Battery capacity, Ah";
  parameter Real battery_voltage_nom = 24.0 "Nominal bus voltage, V";
  parameter Real battery_soc_init = 1.0 "State of charge at t=0, 0..1";
  parameter Real panel_area = 0.5 "Collecting area, m2";
  parameter Real panel_efficiency = 0.28 "Irradiance-to-electrical, 0..1";
  parameter Real irradiance = 1361.0 "Incident irradiance, W/m2 (solar constant)";
  parameter Real motor_rated_power = 2000.0 "Continuous rated shaft power, W";
  parameter Real motor_rated_torque = 20.0 "Shaft torque at full throttle, N.m";
  parameter Real cruise_omega = 8.0 "Nominal shaft speed, rad/s (demo proxy for wheel spin)";

  // Components — the reusable maths, wired on one node.
  Battery bat(voltage_nom = battery_voltage_nom, capacity = battery_capacity, soc_init = battery_soc_init);
  SolarPanel panel(area = panel_area, efficiency = panel_efficiency);
  DCMotor motor(rated_power = motor_rated_power);

  // Boundary — wired by cosim / set by the autopilot.
  input Real sun_azimuth "Sun azimuth, rad (from the solar bridge)";
  input Real panel_yaw "Actual panel yaw, rad (from the SunTracker)";
  input Real vehicle_throttle "Throttle command, -1..1 (from the autopilot)";

  // Outputs — unchanged interface, so the HUD / rhai keep reading the same ports.
  output Real soc_out "State of charge, 0..1";
  output Real voltage_out "Bus terminal voltage, V";
  output Real solar_charging "Charging current from the panel, A";
equation
  // Kirchhoff at the shared bus node writes the balance itself.
  connect(panel.p, bat.p);
  connect(motor.p, bat.p);

  // Panel alignment → cosine of incidence, clamped to the lit hemisphere. The panel's
  // own equation turns this into power and pushes it onto the bus as current.
  panel.irradiance = irradiance;
  panel.cos_incidence = max(cos(sun_azimuth - panel_yaw), 0.0);

  // Throttle → shaft torque; the motor draws what it needs to hold it at cruise speed.
  // No draw at rest: torque is zero, so `mech_power = torque*omega` is zero.
  motor.torque_cmd = motor_rated_torque * abs(vehicle_throttle);
  motor.omega = cruise_omega;

  soc_out = bat.soc_out;
  voltage_out = bat.p.v;
  // `panel.p.i` is negative (current LEAVES the panel into the node); flip the sign so
  // the reported charging current reads positive.
  solar_charging = -panel.p.i;
end SolarRoverPower;
