within LunCo.Pointing;
model EarthTracker "Two-axis high-gain antenna: hold the dish on Earth."
  // An ASSEMBLY, not a new law: two `ServoAxis` instances (the same component
  // the solar tracker uses) plus one `DishPattern`. The only equations written
  // here are the wiring and the geometry that connects them — which is the
  // point of a component library.
  //
  // ── Where the inputs come from ──────────────────────────────────────────
  // Unit direction to Earth in the ANTENNA MOUNT frame, published by the
  // celestial bridge.  The frame is explicit: +X right, +Y up, -Z forward.
  // It is the full inverse mount attitude, so yaw, pitch and roll are all
  // accounted for before this model ever chooses a pair of joint angles.
  input Real earth_mount_x "Earth direction, mount-right";
  input Real earth_mount_y "Earth direction, mount-up";
  input Real earth_mount_z "Earth direction, mount-forward";

  parameter Real tau = 1.5 "gimbal time constant (s)";
  parameter Real diameter = 3.0 "reflector diameter (m) — must match the USD dish";
  parameter Real frequency = 2.2e9 "link frequency (Hz)";

  LunCo.Pointing.ServoAxis azimuth(tau = tau);
  LunCo.Pointing.ServoAxis elevation(tau = tau);
  LunCo.Pointing.DishPattern beam(diameter = diameter, frequency = frequency);

  // Gimbal angles for the two standard USD revolute-joint drives.
  output Real az "dish azimuth setpoint (rad)";
  output Real el "dish elevation setpoint (rad)";
  // Link telemetry for the HUD's COMMS panel.
  output Real point_error "angle between boresight and Earth (rad)";
  output Real gain_frac "fraction of peak gain on the link, 0..1";
  output Real locked "1 while Earth is inside the half-power beam";
equation
  // Standard right-handed USD/Avian axes: positive yaw about +Y sends -Z
  // toward -X, hence the minus on the mount-right component.  Positive pitch
  // about +X sends -Z toward +Y.  This is the one vector→joint convention for
  // every composed antenna; hosts never duplicate a sign or axis conversion.
  azimuth.cmd = atan2(-earth_mount_x, -earth_mount_z);
  elevation.cmd = asin(max(-1.0, min(1.0, earth_mount_y)));
  az = azimuth.angle;
  el = elevation.angle;

  // Tangent-plane separation between boresight and target. The cos(el) factor is
  // the convergence of azimuth lines toward the zenith — a yaw error means
  // less on the sky the higher you point. Stays differentiable at zero, which an
  // acos() formulation does not.
  point_error = sqrt(atan2(sin(azimuth.cmd - az), cos(azimuth.cmd - az))^2
                     + (cos(el) * (elevation.cmd - el))^2);
  beam.point_error = point_error;
  gain_frac = beam.gain_frac;
  locked = beam.locked;
end EarthTracker;
