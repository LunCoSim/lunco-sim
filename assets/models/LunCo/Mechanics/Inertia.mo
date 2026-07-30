within LunCo.Mechanics;
// A rotating mass with a flange at each end: the rotor, the shaft, the wheel disc.
//
// J * der(w) = sum of the torques arriving at both flanges. Two flanges rather than one
// so it can sit IN a driveline rather than only terminate it — motor -> inertia -> gearbox
// is one shaft, not two.
//
// This is the model that finally gives reflected inertia somewhere real to live. Today a
// rotor inertia is authored (`lunco:motor:rotorInertia`), read into Rust, scaled by N^2 —
// and then not applied to anything, because Avian has no armature concept to apply it to.
// Composed here, behind a `GearRatio`, the reflection is not a formula anyone maintains:
// it falls out of the two equation sets being solved together.
model Inertia
  extends LunCo.Icons.Mechanics;
  parameter Real J = 1.0e-4 "Moment of inertia about the axis, kg.m2";
  parameter Real w_init = 0.0 "Initial angular velocity, rad/s";

  Flange flange_a;
  Flange flange_b;
  Real phi "Rotation angle, rad";
  Real w(start = w_init) "Angular velocity, rad/s";
  output Real angle_rad(unit="rad") "Rotating body's angular position";
  output Real speed_rad_s(unit="rad/s") "Rotating body's angular velocity";
  output Real net_torque_nm(unit="N.m") "Net torque accelerating the rotating body";
equation
  flange_a.phi = phi;
  flange_b.phi = phi;
  w = der(phi);
  J * der(w) = flange_a.tau + flange_b.tau;
  angle_rad = phi;
  speed_rad_s = w;
  net_torque_nm = flange_a.tau + flange_b.tau;
end Inertia;
