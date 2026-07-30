within LunCo.Pointing;
model SunTracker "Azimuth sun-tracker: yaw a panel to face the sun."
  extends LunCo.Icons.Pointing;
  // Unit sun direction in the panel mount frame: +X right, +Y up, -Z forward.
  // The environment bridge owns world→mount conversion; this model owns the
  // one right-handed mount-vector→yaw conversion.
  input Real sun_mount_x "Sun direction, mount-right";
  input Real sun_mount_y "Sun direction, mount-up";
  input Real sun_mount_z "Sun direction, mount-forward";
  output Real yaw "panel yaw setpoint (rad)";
  parameter Real tau = 0.2 "tracking time constant (s)";

  LunCo.Pointing.ServoAxis drive(tau = tau);
equation
  // Positive yaw around +Y sends -Z toward -X.
  drive.cmd = atan2(-sun_mount_x, -sun_mount_z);
  yaw = drive.angle;
end SunTracker;
