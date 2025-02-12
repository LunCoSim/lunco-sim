within Mechanical;
model Damper "Linear viscous damper"
  parameter Real d = 1.0 "Damping coefficient (N.s/m)";
  
  MechanicalConnector port_a "Left mechanical connector" annotation(Placement(visible = true));
  MechanicalConnector port_b "Right mechanical connector" annotation(Placement(visible = true));
  
  // Internal variables
  Real velocity_diff "Relative velocity between ports (m/s)";
  
equation
  // Calculate relative velocity
  velocity_diff = port_b.velocity - port_a.velocity;
  
  // Damping force: F = -d*v
  port_a.force = d * velocity_diff;
  port_b.force = -d * velocity_diff;
  
  annotation(
    Documentation(info="Linear viscous damper.
    Parameters:
    - d: Damping coefficient (N.s/m)
    
    The damper applies a force proportional to the relative velocity:
    F = -d*v where v is the relative velocity between ports"),
    Icon(graphics={
      Line(points={{-60,0},{-40,0}}, color={0,0,0}),
      Rectangle(extent={{-40,20},{40,-20}}, fillColor={192,192,192}, fillPattern=FillPattern.Solid),
      Line(points={{40,0},{60,0}}, color={0,0,0}),
      Text(extent={{-100,-40},{100,-60}}, textString="%name")
    })
  );
end Damper; 