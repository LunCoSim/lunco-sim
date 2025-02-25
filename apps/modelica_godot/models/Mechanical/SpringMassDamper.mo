model SpringMassDamper
  "A simple spring-mass-damper system"
  
  // Components
  Mechanical.Mass mass(m=1.0);  // 1 kg mass
  Mechanical.Spring spring(k=10.0);  // Spring constant 10 N/m
  Mechanical.Damper damper(d=0.5);  // Damping coefficient 0.5 N.s/m
  Mechanical.Fixed fixed;  // Fixed point
  
  // Initial conditions
  parameter Real x0 = 0.5 "Initial position in meters";
  parameter Real v0 = 0.0 "Initial velocity in m/s";
  
initial equation
  mass.s = x0;
  mass.v = v0;
  
equation
  // Connect components
  connect(fixed.flange, spring.flange_a);
  connect(spring.flange_b, mass.flange_a);
  connect(fixed.flange, damper.flange_a);
  connect(damper.flange_b, mass.flange_a);

annotation(
  experiment(
    StartTime = 0,
    StopTime = 10,
    Interval = 0.01
  )
);
end SpringMassDamper; 