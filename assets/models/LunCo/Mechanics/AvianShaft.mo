within LunCo.Mechanics;
// Mechanical boundary for a wheel realized by Avian.
//
// The wheel's measured angular speed is an input to the solved shaft. The
// flange flow is the torque required by the connected driveline, exposed as an
// output for the runtime to apply to the wheel bodies. This is a generic
// rotational boundary and contains no motor, battery, or vehicle semantics.
model AvianShaft
  extends LunCo.Icons.Mechanics;
  input Real speed(unit="rad/s") "Measured Avian shaft speed";
  Flange flange;
  output Real torque(unit="N.m") "Torque delivered to the Avian shaft";
  output Real speed_rad_s(unit="rad/s") "Boundary shaft speed";
equation
  der(flange.phi) = speed;
  torque = flange.tau;
  speed_rad_s = speed;
end AvianShaft;
