// tagline: Annotation visual fixture — Engine, Tank, Gimbal icons on a stage diagram
// Visual test fixture for the graphical-annotation pipeline.
//
// Exercises every primitive the slice-1 extractor handles (Rectangle,
// Line, Polygon, Text) plus Placement(transformation(extent, origin,
// rotation)). The composite RocketStage at the bottom drops three
// annotated leaf classes onto a Diagram so the canvas renderer can show
// them side by side.

package AnnotatedRocketStage

  // ── Composite (declared first so the workbench picks it as the
  // ── active class when the file is opened — `extract_model_name`
  // ── returns the first non-package class, and we want users to land
  // ── on the diagram view, not on a leaf equation model) ──
  model RocketStage "Composite — drop the three annotated leaves on a diagram"
    Tank tank
      annotation(Placement(transformation(extent={{-90,20},{-50,80}})));
    Engine engine
      annotation(Placement(transformation(extent={{-20,-40},{20,20}})));
    Gimbal gimbal
      annotation(Placement(transformation(
        extent={{40,-40},{80,0}}, origin={0,0}, rotation=15)));
    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — annotation test",
            textColor={0,0,0}),
          Line(points={{-70,20},{0,20}},
            color={50,50,150}, thickness=0.4),
          Line(points={{20,-10},{60,-20}},
            color={150,50,50}, thickness=0.4)
        }),
      experiment(StartTime=0, StopTime=10, Tolerance=1e-6, Interval=0.01));
  end RocketStage;

  // ── Leaf 1: a rocket engine icon ──
  // Combustion chamber rectangle, bell-nozzle polygon, centre line,
  // and a label. Coordinate system is the MLS default
  // {{-100,-100},{100,100}}.
  model Engine "Liquid rocket engine — visual icon test"
    input Real throttle = 1.0;
    Real thrust;
  equation
    thrust = 1.0e6 * throttle;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        // Combustion chamber
        Rectangle(extent={{-30,60},{30,10}},
          lineColor={40,40,40},
          fillColor={200,200,210},
          fillPattern=FillPattern.Solid,
          lineThickness=0.5),
        // Bell nozzle (polygon: top is chamber base, bottom widens)
        Polygon(points={{-30,10},{30,10},{60,-70},{-60,-70}},
          lineColor={40,40,40},
          fillColor={170,80,40},
          fillPattern=FillPattern.Solid),
        // Centreline
        Line(points={{0,80},{0,-90}},
          color={0,0,0},
          pattern=LinePattern.Dash,
          thickness=0.25),
        // Plume hint (short red line out the bottom)
        Line(points={{-40,-70},{0,-95},{40,-70}},
          color={220,40,30},
          thickness=0.6),
        // Label
        Text(extent={{-80,90},{80,70}},
          textString="Engine",
          textColor={0,0,0})
      }));
  end Engine;

  // ── Leaf 2: a propellant tank ──
  // Body rectangle + dome polygon at the top, label inside.
  model Tank "Propellant tank — visual icon test"
    parameter Real m_initial = 4000.0;
    Real m(start=m_initial);
    input Real m_dot = 0.0;
  equation
    der(m) = -m_dot;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        // Tank body
        Rectangle(extent={{-50,60},{50,-80}},
          lineColor={20,20,20},
          fillColor={210,220,235},
          fillPattern=FillPattern.Solid,
          lineThickness=0.4,
          radius=4),
        // Dome (triangular approximation, no Ellipse yet)
        Polygon(points={{-50,60},{50,60},{0,90}},
          lineColor={20,20,20},
          fillColor={180,195,215},
          fillPattern=FillPattern.Solid),
        // Fill-level indicator
        Rectangle(extent={{-40,40},{40,-70}},
          lineColor={50,80,140},
          fillColor={120,160,220},
          fillPattern=FillPattern.Solid),
        Text(extent={{-50,10},{50,-10}},
          textString="LOX",
          textColor={0,0,80}),
        Text(extent={{-90,-85},{90,-100}},
          textString="Tank",
          textColor={40,40,40})
      }));
  end Tank;

  // ── Leaf 3: a thrust-vector gimbal ──
  // Two crossed lines + a small filled square at the pivot.
  model Gimbal "Thrust-vector gimbal — visual icon test"
    input Real pitch_cmd = 0.0;
    input Real yaw_cmd = 0.0;
    Real pitch;
    Real yaw;
  equation
    pitch = pitch_cmd;
    yaw = yaw_cmd;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        Line(points={{-80,0},{80,0}},
          color={80,80,80}, thickness=0.6),
        Line(points={{0,-80},{0,80}},
          color={80,80,80}, thickness=0.6),
        Polygon(points={{-15,-15},{15,-15},{15,15},{-15,15}},
          lineColor={0,0,0},
          fillColor={255,180,40},
          fillPattern=FillPattern.Solid),
        Text(extent={{-90,90},{90,70}},
          textString="Gimbal",
          textColor={0,0,0})
      }));
  end Gimbal;

end AnnotatedRocketStage;
