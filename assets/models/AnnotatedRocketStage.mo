// tagline: Rocket stage with typed connectors — colors per signal role, OMEdit-style
// Working rocket stage modelled at the signal level.
//
// Causality choice (per discussion in CHANGELOG / chat):
//   * Engine owns the throttle → fuel-demand → thrust map (it knows
//     its Isp and m_dot_max). Throttle is a `RealInput` from outside.
//   * Tank receives the engine's fuel demand and depletes accordingly,
//     publishes its current propellant mass as a measurement signal.
//   * Airframe consumes thrust + current mass, integrates 1-D flight.
//
// Typed connectors per semantic role so wires render in different
// colors (the OMEdit / Dymola convention is to read color from each
// connector class's Icon annotation):
//
//   FuelDemand  — orange, kg/s, command from consumer to source
//   MassSignal  — green,  kg,   measurement / publication
//   ThrustForce — red,    N,    force output
//   RealInput/Output (MSL) — magenta, generic control signal
//
// Wire summary (signal direction, not fluid direction):
//
//   engine.fuel_demand → tank.fuel_demand   (orange — command)
//   tank.mass_out      → airframe.mass_in   (green  — mass measurement)
//   engine.thrust      → airframe.thrust_in (red    — force)

package AnnotatedRocketStage

  // ── Typed connectors ─────────────────────────────────────────────
  // Each connector class carries an `Icon` whose `lineColor` becomes
  // the wire color in standards-compliant editors. Inputs are drawn
  // as a filled square at the icon boundary; outputs as a filled
  // triangle pointing outward. The Modelica.Blocks.Interfaces convention.

  connector FuelDemandOutput = output Real(unit = "kg/s")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {220,140,40},
        fillColor = {235,170,70},
        fillPattern = FillPattern.Solid)}));

  connector FuelDemandInput = input Real(unit = "kg/s")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {220,140,40},
        fillColor = {235,170,70},
        fillPattern = FillPattern.Solid)}));

  connector MassSignalOutput = output Real(unit = "kg")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {40,140,60},
        fillColor = {80,180,100},
        fillPattern = FillPattern.Solid)}));

  connector MassSignalInput = input Real(unit = "kg")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {40,140,60},
        fillColor = {80,180,100},
        fillPattern = FillPattern.Solid)}));

  connector ThrustForceOutput = output Real(unit = "N")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {180,40,40},
        fillColor = {220,70,70},
        fillPattern = FillPattern.Solid)}));

  connector ThrustForceInput = input Real(unit = "N")
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Polygon(
        points = {{-100,100},{100,0},{-100,-100}},
        lineColor = {180,40,40},
        fillColor = {220,70,70},
        fillPattern = FillPattern.Solid)}));

  // ── Composite (declared first so the workbench picks it as the
  // ── active class when the file is opened) ───────────────────────
  model RocketStage "Single-stage rocket — engine demands fuel, tank delivers"
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

    // Three connects — colors come from each connector's Icon:
    //   orange = fuel-flow command, green = mass measurement, red = thrust force.
    connect(engine.fuel_demand, tank.fuel_demand);
    connect(tank.mass_out, airframe.mass_in);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — typed connectors",
            textColor={0,0,0})
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  // ── Liquid rocket engine ──
  // Owns the throttle → m_dot map and the Isp → thrust calculation.
  // Publishes m_dot as a fuel-demand signal that the tank consumes;
  // publishes thrust as a force signal that the airframe consumes.
  model Engine "Liquid rocket engine — constant-Isp model"
    parameter Real Isp = 300 "Specific impulse (s)";
    parameter Real g0 = 9.81 "Standard gravity (m/s^2)";
    parameter Real m_dot_max = 20 "Max propellant flow at full throttle (kg/s)";

    Modelica.Blocks.Interfaces.RealInput throttle "Throttle command [0..1]"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    FuelDemandOutput fuel_demand "Demanded propellant flow (kg/s)"
      annotation(Placement(transformation(extent={{-10,100},{10,120}})));
    ThrustForceOutput thrust "Thrust (N)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    fuel_demand = throttle * m_dot_max;
    thrust = Isp * g0 * fuel_demand;
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
  // Releases the fuel rate the engine demands; publishes current mass.
  model Tank "Propellant tank — depletes at the demanded rate"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    Real m(start=m_initial, fixed=true) "Propellant mass (kg)";

    FuelDemandInput fuel_demand "Outflow rate demanded by consumer (kg/s)"
      annotation(Placement(transformation(extent={{-10,-120},{10,-100}})));
    MassSignalOutput mass_out "Current propellant mass (kg)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    der(m) = -fuel_demand;
    mass_out = m;
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

    ThrustForceInput thrust_in "Thrust from engine (N)"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    MassSignalInput mass_in "Current propellant mass (kg)"
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
    Modelica.Blocks.Interfaces.RealInput pitch_cmd
      annotation(Placement(transformation(extent={{-120,40},{-100,60}})));
    Modelica.Blocks.Interfaces.RealInput yaw_cmd
      annotation(Placement(transformation(extent={{-120,-60},{-100,-40}})));
    Modelica.Blocks.Interfaces.RealOutput pitch
      annotation(Placement(transformation(extent={{100,40},{120,60}})));
    Modelica.Blocks.Interfaces.RealOutput yaw
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
