model ConnectionWorkspaceProbe
  "Bounded changing signal for connection authoring and inspection acceptance"
  input Real feedback;
  output Real signal;
equation
  signal = 2 + sin(time);
  annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={
    Rectangle(extent={{-80,60},{80,-60}}, lineColor={0,0,127},
      fillColor={230,230,255}, fillPattern=FillPattern.Solid),
    Line(points={{-60,-20},{-30,30},{0,-20},{30,30},{60,-20}}, color={0,0,127})
  }));
end ConnectionWorkspaceProbe;
