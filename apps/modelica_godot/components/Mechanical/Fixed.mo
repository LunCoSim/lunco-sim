within ModelicaGodot.Mechanical;
model Fixed "Fixed point in space (ground)"
  parameter Real position = 0.0 "Fixed position (m)";
  
  MechanicalConnector port "Mechanical connector" annotation(Placement(visible = true));
  
equation
  // Fixed position
  port.position = position;
  port.velocity = 0;
  
  annotation(
    Documentation(info="Fixed point in space (ground reference).
    Parameters:
    - position: Fixed position in space (m)
    
    This component represents a fixed point in space that can be used
    as a reference point or ground. The position is constant and the
    velocity is always zero. The force is calculated to maintain the
    fixed position."),
    Icon(graphics={
      Line(points={{-100,0},{0,0}}, color={0,0,0}),
      Line(points={{-10,-10},{10,-10}}, color={0,0,0}),
      Line(points={{-20,-20},{20,-20}}, color={0,0,0}),
      Line(points={{-30,-30},{30,-30}}, color={0,0,0}),
      Text(extent={{-100,-40},{100,-60}}, textString="%name")
    })
  );
end Fixed; 