within Mechanical;
model Spring "Linear 1D spring"
  parameter Real k = 1.0 "Spring constant (N/m)";
  parameter Real l0 = 1.0 "Natural length (m)";
  
  MechanicalConnector port_a "Left mechanical connector" annotation(Placement(visible = true));
  MechanicalConnector port_b "Right mechanical connector" annotation(Placement(visible = true));
  
  // Internal variables
  Real length "Current length of spring (m)";
  Real elongation "Elongation from natural length (m)";
  
equation
  // Calculate current length and elongation
  length = port_b.position - port_a.position;
  elongation = length - l0;
  
  // Hooke's law: F = -k*x
  port_a.force = k * elongation;
  port_b.force = -k * elongation;
  
  annotation(
    Documentation(info="Linear 1D spring following Hooke's law.
    Parameters:
    - k: Spring constant (N/m)
    - l0: Natural length (m)
    
    The spring follows Hooke's law (F = -k*x) where:
    - x is the elongation from natural length
    - positive force means pushing
    - forces on both ports are equal and opposite"),
    Icon(graphics={
      Line(points={{-60,0},{-40,0}}, color={0,0,0}),
      Line(points={{-40,0},{-30,20},{-10,-20},{10,20},{30,-20},{40,0}}, color={0,0,0}),
      Line(points={{40,0},{60,0}}, color={0,0,0}),
      Text(extent={{-100,-40},{100,-60}}, textString="%name")
    })
  );
end Spring; 