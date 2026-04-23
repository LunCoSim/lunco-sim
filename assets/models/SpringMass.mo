// tagline: Mass–spring–damper second-order system
model SpringMass
  parameter Real m = 1.0 "Mass in kg";
  parameter Real k = 10.0 "Spring constant in N/m";
  parameter Real d = 0.5 "Damping coefficient in Ns/m";
  
  Real x(start=1.0) "Position in m";
  Real v(start=0.0) "Velocity in m/s";

equation
  v = der(x);
  m * der(v) = -k * x - d * v;
end SpringMass;
