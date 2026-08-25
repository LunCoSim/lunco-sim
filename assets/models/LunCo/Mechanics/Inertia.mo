within LunCo.Mechanics;
// A rotating mass with a flange at each end: a shaft, wheel disc, or other
// authored rotating assembly.
//
// J * der(w) = sum of the torques arriving at both flanges. Two flanges rather than one
// so it can sit IN a driveline rather than only terminate it — source -> inertia -> gearbox
// is one shaft, not two.
//
// This is the reusable rotational mass boundary. Its sole state is the authored
// assembly inertia; domain components that produce torque do not duplicate it.
model Inertia
  extends LunCo.Icons.Mechanics;
  parameter Real J = 1.0e-4 "Moment of inertia about the axis, kg.m2";
  parameter Real w_init = 0.0 "Initial angular velocity, rad/s";

  Flange flange_a;
  Flange flange_b;
  Real phi "Rotation angle, rad";
  Real w(start = w_init) "Angular velocity, rad/s";
equation
  flange_a.phi = phi;
  flange_b.phi = phi;
  w = der(phi);
  J * der(w) = flange_a.tau + flange_b.tau;
end Inertia;
