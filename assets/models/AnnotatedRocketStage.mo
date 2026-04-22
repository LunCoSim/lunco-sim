// tagline: Rocket stage with real connectors — tank→engine fuel, engine→airframe thrust
// A working rocket stage wired with Modelica connectors.
//
// Tank exposes an outflow RealInput (m_dot_in) and a mass RealOutput
// (m_out). Engine has throttle input, m_dot_out, and thrust outputs.
// Airframe consumes thrust and current mass to integrate 1-D vertical
// dynamics. RocketStage instantiates them and uses three `connect()`
// statements — visible as wires in the Diagram — to hook up:
//
//   engine.m_dot_out  →  tank.m_dot_in    (propellant demand)
//   tank.m_out        →  airframe.mass_in (current vehicle mass)
//   engine.thrust     →  airframe.thrust_in

package AnnotatedRocketStage

  // Minimal signal connectors. MLS §9.1 says a connector with only
  // a single `input Real` (resp. `output`) variable is a valid
  // causal signal port; `connect(out, in)` then generates `out = in`.
  connector RealInput = input Real;
  connector RealOutput = output Real;

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
    // Without this guard the engine keeps drawing, driving tank.m
    // negative.
    engine.throttle = if tank.m > 1.0 then 1.0 else 0.0;

    // Three real connects — these are the wires you see on the diagram.
    connect(engine.m_dot_out, tank.m_dot_in);
    connect(tank.m_out, airframe.mass_in);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — real connectors",
            textColor={0,0,0}),
          // Tank bottom → Engine top (propellant demand)
          Line(points={{-70,20},{0,30}},
            color={50,50,150}, thickness=0.6),
          Text(extent={{-60,24},{-10,34}},
            textString="m_dot",
            textColor={50,50,150}),
          // Tank bottom → Airframe (current vehicle mass)
          Line(points={{-70,20},{-70,-60},{45,-20}},
            color={80,120,50}, thickness=0.4,
            pattern=LinePattern.Dash),
          Text(extent={{-50,-52},{0,-42}},
            textString="mass",
            textColor={80,120,50}),
          // Engine → Airframe (thrust vector)
          Line(points={{25,0},{45,0}},
            color={150,50,50}, thickness=0.6),
          Text(extent={{27,4},{45,14}},
            textString="thrust",
            textColor={150,50,50})
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  // ── Liquid rocket engine ──
  // Constant-Isp, linear-throttle model. Exposes thrust and propellant
  // flow as RealOutput signals so the airframe and tank can consume
  // them via connect().
  model Engine "Liquid rocket engine — constant-Isp model"
    parameter Real Isp = 300 "Specific impulse (s)";
    parameter Real g0 = 9.81 "Standard gravity (m/s^2)";
    parameter Real m_dot_max = 20 "Max propellant flow (kg/s)";
    RealInput throttle "Throttle command [0..1]";
    RealOutput thrust "Thrust (N)";
    RealOutput m_dot_out "Propellant consumption (kg/s)";
  equation
    m_dot_out = throttle * m_dot_max;
    thrust = Isp * g0 * m_dot_out;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        // Combustion chamber
        Rectangle(extent={{-30,60},{30,10}},
          lineColor={40,40,40},
          fillColor={200,200,210},
          fillPattern=FillPattern.Solid,
          lineThickness=0.5),
        // Bell nozzle
        Polygon(points={{-30,10},{30,10},{60,-70},{-60,-70}},
          lineColor={40,40,40},
          fillColor={170,80,40},
          fillPattern=FillPattern.Solid),
        // Centreline
        Line(points={{0,80},{0,-90}},
          color={0,0,0},
          pattern=LinePattern.Dash,
          thickness=0.25),
        // Plume hint
        Line(points={{-40,-70},{0,-95},{40,-70}},
          color={220,40,30},
          thickness=0.6),
        Text(extent={{-80,90},{80,70}},
          textString="Engine",
          textColor={0,0,0})
      }));
  end Engine;

  // ── Propellant tank ──
  // der(m) = -m_dot_in. Exposes m as a RealOutput so the airframe can
  // read it through a connect(); exposes m_dot_in as the demanded flow
  // (driven by the engine).
  model Tank "Propellant tank — constant-density mass reservoir"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    Real m(start=m_initial, fixed=true) "Propellant mass (kg)";
    RealInput m_dot_in "Outflow demanded by consumer (kg/s)";
    RealOutput m_out "Current mass (kg)";
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
  // Consumes thrust and current total mass (dry + propellant), integrates
  // 1-D vertical flight. The dry_mass parameter is added inside — the
  // `mass_in` connector carries propellant mass only.
  model Airframe "Vehicle body — 1-D vertical flight dynamics"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";
    RealInput thrust_in "Thrust from engine (N)";
    RealInput mass_in "Current propellant mass (kg)";
    Real altitude(start=0, fixed=true) "m";
    Real velocity(start=0, fixed=true) "m/s";
    Real total_mass "kg";
  equation
    total_mass = dry_mass + mass_in;
    der(altitude) = velocity;
    der(velocity) = (thrust_in - total_mass * g) / total_mass;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        // Rocket fuselage
        Rectangle(extent={{-20,-70},{20,60}},
          lineColor={40,40,40},
          fillColor={230,230,230},
          fillPattern=FillPattern.Solid,
          lineThickness=0.5,
          radius=4),
        // Nose cone
        Polygon(points={{-20,60},{20,60},{0,95}},
          lineColor={40,40,40},
          fillColor={200,210,230},
          fillPattern=FillPattern.Solid),
        // Fins
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

  // ── Gimbal (kept as a decorative class for future thrust-vector work;
  // ── not instantiated in RocketStage yet) ──
  model Gimbal "Thrust-vector gimbal — visual icon only (not wired)"
    RealInput pitch_cmd "Commanded pitch (rad)";
    RealInput yaw_cmd "Commanded yaw (rad)";
    RealOutput pitch "Effective pitch (rad)";
    RealOutput yaw "Effective yaw (rad)";
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
