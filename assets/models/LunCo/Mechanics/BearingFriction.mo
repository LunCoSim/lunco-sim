within LunCo.Mechanics;
// Grounded linear viscous bearing: positive flange power is absorbed as heat.
// No Coulomb friction or stiction is represented. Do not add this component to
// a wheel that already realizes the same authored damping in the native solver.
model BearingFriction
  extends LunCo.Icons.Mechanics;
  parameter Real d(unit="N.m.s/rad") = 0.0 "Viscous damping coefficient, N.m per rad/s";

  Flange flange;
  Real w "Angular velocity, rad/s";
  output Real speed_rad_s(unit="rad/s") "Bearing shaft angular velocity";
  output Real resisting_torque_nm(unit="N.m") "Viscous resisting torque applied by the bearing";
  output Real heat(unit="W") "Bearing dissipation available to a thermal HeatLoad";
equation
  assert(d >= 0.0, "BearingFriction requires nonnegative viscous damping");
  w = der(flange.phi);
  flange.tau = d * w;
  speed_rad_s = w;
  resisting_torque_nm = flange.tau;
  heat = flange.tau * w;
end BearingFriction;
