// tagline: Rocket stage with acausal fuel pipe — fuel flows tank → engine, signals stay causal
// Working rocket stage with mixed acausal/causal connections.
//
// Fuel is modelled with an *acausal* `FuelPort` connector carrying a
// flow variable `m_dot` (kg/s). The single `connect(engine.fuel_in,
// tank.fuel_out)` is the literal fuel pipe — mass conservation
// (Σ m_dot = 0 at the connection) is generated automatically by the
// compiler. The wire on the diagram has no arrowhead because flow
// direction is determined by the solver, not by the author.
//
// Mass and thrust stay causal (one-way signals): tank publishes its
// current mass, engine publishes thrust, both consumed by airframe.
// Causal wires render with arrowheads at the input end.
//
//   engine.fuel_in   ↔  tank.fuel_out      (acausal — fuel pipe, teal)
//   tank.mass_out    →  airframe.mass_in   (causal — green)
//   engine.thrust    →  airframe.thrust_in (causal — red)

package AnnotatedRocketStage

  // ── Acausal fuel-pipe connector ───────────────────────────────────
  // Single class shared by both ends; `flow` makes m_dot a
  // through-variable so the solver enforces mass conservation at
  // every connection point automatically.
  connector FuelPort "Acausal fuel-line port — pressure (potential) + m_dot (flow)"
    // MLS §9.3.1 requires a balanced connector: one potential per
    // flow variable. `p` is the line pressure (anchored by the
    // tank); `m_dot` is the through-variable that conserves to zero
    // at every connection point.
    Real p(unit = "Pa");
    flow Real m_dot(unit = "kg/s");
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Ellipse(
        extent = {{-100,-100},{100,100}},
        lineColor = {40,120,150},
        fillColor = {70,160,180},
        fillPattern = FillPattern.Solid)}));
  end FuelPort;

  // ── Causal signal connectors (one class per role) ────────────────
  // MSL convention: separate input/output connector classes per role
  // so the type system can enforce output→input pairing on connect().

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
  model RocketStage "Single-stage rocket — engine pulls fuel from tank, signals to airframe"
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

    // The fuel pipe — acausal connection. Mass conservation
    // (engine.fuel_in.m_dot + tank.fuel_out.m_dot = 0) is generated
    // by the connect-set rule; the solver decides which way mass
    // actually moves based on each component's equations.
    connect(engine.fuel_in, tank.fuel_out);

    // Causal information signals.
    connect(tank.mass_out, airframe.mass_in);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — acausal fuel + causal signals",
            textColor={0,0,0})
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  // ── Liquid rocket engine ──
  // Engine is the *consumer* in the fuel network: throttle command
  // sets m_dot, sign convention is "flow into the connector point
  // from this component is positive". Engine is sucking fuel out of
  // the line, so engine.fuel_in.m_dot is negative.
  model Engine "Liquid rocket engine — consumer on the fuel pipe"
    parameter Real Isp = 300 "Specific impulse (s)";
    parameter Real g0 = 9.81 "Standard gravity (m/s^2)";
    parameter Real m_dot_max = 20 "Max propellant flow at full throttle (kg/s)";

    Modelica.Blocks.Interfaces.RealInput throttle "Throttle command [0..1]"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    FuelPort fuel_in "Acausal fuel intake"
      annotation(Placement(transformation(extent={{-10,100},{10,120}})));
    ThrustForceOutput thrust "Thrust (N)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    // Engine sucks at -throttle*m_dot_max (negative = consumer).
    fuel_in.m_dot = -throttle * m_dot_max;
    // Thrust uses the magnitude of the consumed flow.
    thrust = Isp * g0 * (-fuel_in.m_dot);
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
  // Tank is the *supplier* in the fuel network. Whatever flow leaves
  // the line through fuel_out (m_dot positive at the supplier side)
  // depletes the stored mass. The actual rate is determined by the
  // engine's equation via the connect-set's mass-balance.
  model Tank "Propellant tank — supplier on the fuel pipe"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    parameter Real p_supply = 2.0e6 "Pressurised supply line (Pa)";
    Real m(start=m_initial, fixed=true) "Propellant mass (kg)";

    FuelPort fuel_out "Acausal fuel outlet"
      annotation(Placement(transformation(extent={{-10,-120},{10,-100}})));
    MassSignalOutput mass_out "Current propellant mass (kg)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    // Tank anchors the line pressure (potential). Engine doesn't
    // constrain p, so the connect-set propagates p_supply to the
    // engine side automatically.
    fuel_out.p = p_supply;
    // Mass leaves the tank at whatever rate is flowing out the port.
    // fuel_out.m_dot is "flow INTO the connector from the tank" — a
    // positive value means the tank is releasing mass.
    der(m) = -fuel_out.m_dot;
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
