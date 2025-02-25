within ModelicaGodot.Mechanical;
model Mass "Point mass with one mechanical connector"
  parameter Real m = 1.0 "Mass of the component (kg)";
  parameter Real x0 = 0.0 "Initial position (m)";
  parameter Real v0 = 0.0 "Initial velocity (m/s)";
  
  MechanicalConnector port "Mechanical connector" annotation(Placement(visible = true));
  
  // Internal variables
  Real acceleration "Acceleration of the mass (m/s²)";
  
initial equation
  port.position = x0;
  port.velocity = v0;
  
equation
  // Newton's second law: F = ma
  m * acceleration = port.force;
  
  // Kinematic relationships
  der(port.position) = port.velocity;
  der(port.velocity) = acceleration;
  
  annotation(
    Documentation(info="Simple 1D point mass with one mechanical connector.
    Parameters:
    - m: Mass of the component (kg)
    - x0: Initial position (m)
    - v0: Initial velocity (m/s)
    
    The component follows Newton's second law of motion (F = ma)
    and basic kinematic relationships between position,
    velocity, and acceleration."),
    Icon(graphics={
      Rectangle(extent={{-50,50},{50,-50}}, lineColor={0,0,0}, fillColor={0,127,255}, fillPattern=FillPattern.Solid),
      Text(extent={{-100,-50},{100,-70}}, textString="%name")
    })
  );
end Mass; 