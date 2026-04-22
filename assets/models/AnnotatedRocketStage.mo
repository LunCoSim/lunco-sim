// tagline: Rocket stage with MSL connectors — ports + connect() wires in the diagram
// A working rocket stage wired with real Modelica connectors.
//
// Tank / Engine / Airframe each declare Modelica.Blocks.Interfaces.Real[In|Out]put
// connectors with Placement annotations, so ports show on the icon
// boundary and `connect()` statements render as wires that follow the
// components when the user drags them on the canvas.
//
//   engine.m_dot_out  →  tank.m_dot_in    (propellant demand)
//   tank.m_out        →  airframe.mass_in (current vehicle mass)
//   engine.thrust     →  airframe.thrust_in

package AnnotatedRocketStage

  // ── Composite (declared first so the workbench picks it as the
  // ── active class when the file is opened — `extract_model_name`
  // ── returns the first non-package class) ──
  model RocketStage "Single-stage rocket — tank, engine, airframe wired by connectors"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";

    Tank tank(m_initial = 4000)
      annotation(Placement(transformation(extent={{-90,20},{-50,80}})));
    Engine engine
      annotation(Placement(transformation(extent={{-25,-30},{25,30}})));
    Airframe airframe(g = g, dry_mass = dry_mass)
      annotation(Placement(transformation(extent={{45,-30},{90,30}})));

  equation
    // Open-loop throttle: full until the tank is effectively empty.
    engine.throttle = if tank.m > 1.0 then 1.0 else 0.0;

    // Three real connects — these produce the diagram wires that
    // track the components when you drag them on the canvas.
    connect(engine.m_dot_out, tank.m_dot_in);
    connect(tank.m_out, airframe.mass_in);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — real connectors",
            textColor={0,0,0})
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  // ── Liquid rocket engine ──
  model Engine "Liquid rocket engine — constant-Isp model"
    parameter Real Isp = 300 "Specific impulse (s)";
    parameter Real g0 = 9.81 "Standard gravity (m/s^2)";
    parameter Real m_dot_max = 20 "Max propellant flow (kg/s)";

    Modelica.Blocks.Interfaces.RealInput throttle "Throttle command [0..1]"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    Modelica.Blocks.Interfaces.RealOutput thrust "Thrust (N)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
    Modelica.Blocks.Interfaces.RealOutput m_dot_out "Propellant consumption (kg/s)"
      annotation(Placement(transformation(extent={{-10,100},{10,120}})));
  equation
    m_dot_out = throttle * m_dot_max;
    thrust = Isp * g0 * m_dot_out;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        Rectangle(extent={{-30,60},{30,10}},
          lineColor={40,40,40},
          fillColor={200,200,210},
          fillPattern=FillPattern.Solid,
          lineThickness=0.5),
        Polygon(points={{-30,10},{30,10},{60,-70},{-60,-70}},
          lineColor={40,40,40},
          fillColor={170,80,40},
          fillPattern=FillPattern.Solid),
        Line(points={{0,80},{0,-90}},
          color={0,0,0}, pattern=LinePattern.Dash, thickness=0.25),
        Line(points={{-40,-70},{0,-95},{40,-70}},
          color={220,40,30}, thickness=0.6),
        Text(extent={{-80,90},{80,70}},
          textString="Engine",
          textColor={0,0,0})
      }));
  end Engine;

  // ── Propellant tank ──
  model Tank "Propellant tank — constant-density mass reservoir"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    Real m(start=m_initial, fixed=true) "Propellant mass (kg)";

    Modelica.Blocks.Interfaces.RealInput m_dot_in "Outflow demanded by consumer (kg/s)"
      annotation(Placement(transformation(extent={{-10,-120},{10,-100}})));
    Modelica.Blocks.Interfaces.RealOutput m_out "Current mass (kg)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    der(m) = -m_dot_in;
    m_out = m;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        Rectangle(extent={{-50,60},{50,-80}},
          lineColor={20,20,20},
          fillColor={210,220,235},
          fillPattern=FillPattern.Solid,
          lineThickness=0.4,
          radius=4),
        Polygon(points={{-50,60},{50,60},{0,90}},
          lineColor={20,20,20},
          fillColor={180,195,215},
          fillPattern=FillPattern.Solid),
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

  // ── Airframe / vehicle body ──
  model Airframe "Vehicle body — 1-D vertical flight dynamics"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";

    Modelica.Blocks.Interfaces.RealInput thrust_in "Thrust from engine (N)"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    Modelica.Blocks.Interfaces.RealInput mass_in "Current propellant mass (kg)"
      annotation(Placement(transformation(extent={{-120,40},{-100,60}})));

    Real altitude(start=0, fixed=true) "m";
    Real velocity(start=0, fixed=true) "m/s";
    Real total_mass "kg";
  equation
    total_mass = dry_mass + mass_in;
    der(altitude) = velocity;
    der(velocity) = (thrust_in - total_mass * g) / total_mass;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        Rectangle(extent={{-20,-70},{20,60}},
          lineColor={40,40,40},
          fillColor={230,230,230},
          fillPattern=FillPattern.Solid,
          lineThickness=0.5,
          radius=4),
        Polygon(points={{-20,60},{20,60},{0,95}},
          lineColor={40,40,40},
          fillColor={200,210,230},
          fillPattern=FillPattern.Solid),
        Polygon(points={{-20,-70},{-45,-90},{-20,-50}},
          lineColor={40,40,40},
          fillColor={160,60,60},
          fillPattern=FillPattern.Solid),
        Polygon(points={{20,-70},{45,-90},{20,-50}},
          lineColor={40,40,40},
          fillColor={160,60,60},
          fillPattern=FillPattern.Solid),
        Text(extent={{-60,10},{60,-10}},
          textString="Airframe",
          textColor={0,0,0})
      }));
  end Airframe;

  // ── Gimbal (decorative — not instantiated in RocketStage) ──
  model Gimbal "Thrust-vector gimbal — visual icon only (not wired)"
    Modelica.Blocks.Interfaces.RealInput pitch_cmd "Commanded pitch (rad)"
      annotation(Placement(transformation(extent={{-120,40},{-100,60}})));
    Modelica.Blocks.Interfaces.RealInput yaw_cmd "Commanded yaw (rad)"
      annotation(Placement(transformation(extent={{-120,-60},{-100,-40}})));
    Modelica.Blocks.Interfaces.RealOutput pitch "Effective pitch (rad)"
      annotation(Placement(transformation(extent={{100,40},{120,60}})));
    Modelica.Blocks.Interfaces.RealOutput yaw "Effective yaw (rad)"
      annotation(Placement(transformation(extent={{100,-60},{120,-40}})));
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
