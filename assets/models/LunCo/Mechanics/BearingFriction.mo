within LunCo.Mechanics;
// Viscous drag between a shaft and its housing: tau = -d * w.
//
// Linear only, deliberately. Coulomb (stick-slip) friction needs an event at every zero
// crossing of `w`, and a driveline that stops and restarts every time a rover changes
// direction would spend its budget on those events. The wheel that actually stops is
// resolved by Avian's contact solver, which already models stiction where it matters — at
// the ground, not in the bearing.
//
// `physxVehicleWheel:dampingRate` is the authored spelling of `d`.
model BearingFriction
  extends LunCo.Icons.Mechanics;
  parameter Real d = 0.0 "Viscous damping coefficient, N.m per rad/s";

  Flange flange;
  Real w "Angular velocity, rad/s";
equation
  w = der(flange.phi);
  flange.tau = d * w;
end BearingFriction;
