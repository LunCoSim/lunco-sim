within ModelicaGodot.Mechanical;
model DampingMassTest
  "Test model for a mass with damping"
  
  // Components
  Mass mass(m=1.0);  // 1 kg mass
  Damper damper(d=0.5);  // Damping coefficient 0.5 N.s/m
  Fixed fixed;  // Fixed point
  
  // Initial conditions
  parameter Real x0 = 1.0 "Initial position in meters";
  parameter Real v0 = 0.0 "Initial velocity in m/s";
  
initial equation
  mass.port.position = x0;
  mass.port.velocity = v0;
  
equation
  // Connect components
  connect(fixed.port, damper.port_a);
  connect(damper.port_b, mass.port);

annotation(
  experiment(
    StartTime = 0,
    StopTime = 10,
    Interval = 0.1
  )
);
end DampingMassTest; 