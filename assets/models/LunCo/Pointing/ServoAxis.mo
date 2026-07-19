within LunCo.Pointing;
model ServoAxis "One first-order pointing axis: ease an angle onto its command."
  // The shared mechanism behind every tracker in this package. A real gimbal
  // (or a panel drive) does not snap to its setpoint — it eases on with a
  // characteristic time. That is one line of ODE, and it is the SAME line for
  // a solar panel's yaw and a dish's elevation, so it lives here once.
  //
  // Used as a component:
  //   LunCo.Pointing.ServoAxis azimuth(tau = 1.5);
  //   equation azimuth.cmd = earth_azimuth;
  input Real cmd "commanded angle (rad)";
  output Real angle(start = 0.0) "actual angle (rad)";

  parameter Real tau = 1.5 "time constant (s) — small is snappy, large is lazy";
equation
  der(angle) = (cmd - angle) / tau;
end ServoAxis;
