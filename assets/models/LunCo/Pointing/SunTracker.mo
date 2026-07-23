within LunCo.Pointing;
model SunTracker "Azimuth sun-tracker: yaw a panel to face the sun."
  input Real sun_azimuth "commanded sun azimuth (rad)";
  output Real yaw "panel yaw setpoint (rad)";
  parameter Real tau = 0.2 "tracking time constant (s)";

  LunCo.Pointing.ServoAxis drive(tau = tau);
equation
  drive.cmd = sun_azimuth;
  yaw = drive.angle;
end SunTracker;
