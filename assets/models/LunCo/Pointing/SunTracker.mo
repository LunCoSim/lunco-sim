within LunCo.Pointing;
model SunTracker "Azimuth sun-tracker: yaw a panel to face the sun."
  extends LunCo.Icons.Pointing;
  // Unit direction to the selected target in the panel mount frame: +X right,
  // +Y up, -Z forward. The environment wire chooses the target; this model
  // owns the right-handed mount-vector→yaw conversion.
  input Real target_mount_x "Target direction, mount-right";
  input Real target_mount_y "Target direction, mount-up";
  input Real target_mount_z "Target direction, mount-forward";
  output Real yaw "panel yaw setpoint (rad)";
  parameter Real tau = 0.2 "tracking time constant (s)";

  LunCo.Pointing.ServoAxis drive(tau = tau);
equation
  // Positive yaw around +Y sends -Z toward -X.
  drive.cmd = atan2(-target_mount_x, -target_mount_z);
  yaw = drive.angle;
end SunTracker;
