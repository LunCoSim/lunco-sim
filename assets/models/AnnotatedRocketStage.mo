// tagline: Rocket stage with acausal fluid — pressurised tank, throttle valve, engine
//
//   Tank  ──FluidPort──► Valve ──FluidPort──► Engine
//      │                   ▲                    │
//      │                   │                    ▼
//    mass_out          throttle              thrust
//      │                                        │
//      └──────► Airframe ◄─────────────────────┘

package AnnotatedRocketStage

  package LunCoAnnotations "Vendor-specific metadata records for the LunCo toolchain"
    record PlotNode "Region in the diagram canvas that hosts a live-signal plot"
      Real extent[2,2] "Diagram-coordinate rectangle of the plot region";
      String signal "Fully qualified Modelica variable to plot";
      String title "Human-readable plot title";
    end PlotNode;
  end LunCoAnnotations;

  partial connector FluidPort "Acausal fluid port (pressure + mass-flow)"
    Real p(unit = "Pa") "Line pressure";
    flow Real m_flow(unit = "kg/s") "Mass flow into the connector from this component";
  end FluidPort;

  connector FluidPort_a "Fluid port — supplier-side appearance"
    extends FluidPort;
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Ellipse(
        extent = {{-100,-100},{100,100}},
        lineColor = {40,120,150},
        fillColor = {70,160,180},
        fillPattern = FillPattern.Solid)}));
  end FluidPort_a;

  connector FluidPort_b "Fluid port — consumer-side appearance"
    extends FluidPort;
    annotation(Icon(coordinateSystem(extent = {{-100,-100},{100,100}}),
      graphics = {Ellipse(
        extent = {{-100,-100},{100,100}},
        lineColor = {40,120,150},
        fillColor = {220,235,240},
        fillPattern = FillPattern.Solid)}));
  end FluidPort_b;

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

  model RocketStage "Single-stage rocket — pressurised tank, throttle valve, engine"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";

    Tank tank(m_initial = 4000, p_supply = 3.0e6)
      annotation(Placement(transformation(extent={{-95,20},{-55,80}})));
    Valve valve(m_flow_max = 100)
      annotation(Placement(transformation(extent={{-40,-20},{0,20}})));
    Engine engine(p_chamber = 1.0e5)
      annotation(Placement(transformation(extent={{15,-30},{55,30}})));
    Airframe airframe(g = g, dry_mass = dry_mass)
      annotation(Placement(transformation(extent={{70,-30},{110,30}})));

  equation
    connect(tank.port, valve.port_a);
    connect(valve.port_b, engine.port);
    connect(tank.mass_out, airframe.mass_in);
    connect(tank.availability, valve.availability);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,98},{100,90}},
            textString="AnnotatedRocketStage — pressurised fluid line with throttle valve",
            fontSize=8,
            textColor={0,0,0}),
          Rectangle(extent={{-100,-50},{-60,-90}},
            lineColor={120,120,120},
            fillColor={245,245,250},
            fillPattern=FillPattern.Solid,
            radius=6),
          Text(extent={{-100,-52},{-60,-60}},
            textString="Tank mass",
            fontSize=6,
            textColor={60,60,60}),
          Rectangle(extent={{-58,-50},{-18,-90}},
            lineColor={120,120,120},
            fillColor={245,245,250},
            fillPattern=FillPattern.Solid,
            radius=6),
          Text(extent={{-58,-52},{-18,-60}},
            textString="Altitude",
            fontSize=6,
            textColor={60,60,60}),
          Rectangle(extent={{-16,-50},{24,-90}},
            lineColor={120,120,120},
            fillColor={245,245,250},
            fillPattern=FillPattern.Solid,
            radius=6),
          Text(extent={{-16,-52},{24,-60}},
            textString="Velocity",
            fontSize=6,
            textColor={60,60,60}),
          Rectangle(extent={{26,-50},{66,-90}},
            lineColor={120,120,120},
            fillColor={245,245,250},
            fillPattern=FillPattern.Solid,
            radius=6),
          Text(extent={{26,-52},{66,-60}},
            textString="Thrust",
            fontSize=6,
            textColor={60,60,60}),
          Rectangle(extent={{68,-50},{108,-90}},
            lineColor={120,120,120},
            fillColor={245,245,250},
            fillPattern=FillPattern.Solid,
            radius=6),
          Text(extent={{68,-52},{108,-60}},
            textString="Acceleration",
            fontSize=6,
            textColor={60,60,60})
        }),
      __LunCo(
        plotNodes={
          LunCoAnnotations.PlotNode(
            extent={{-100,-50},{-60,-90}},
            signal="tank.m",
            title="Tank mass"),
          LunCoAnnotations.PlotNode(
            extent={{-58,-50},{-18,-90}},
            signal="airframe.altitude",
            title="Altitude"),
          LunCoAnnotations.PlotNode(
            extent={{-16,-50},{24,-90}},
            signal="airframe.velocity",
            title="Velocity"),
          LunCoAnnotations.PlotNode(
            extent={{26,-50},{66,-90}},
            signal="airframe.thrust_in",
            title="Thrust"),
          LunCoAnnotations.PlotNode(
            extent={{68,-50},{108,-90}},
            signal="airframe.acceleration",
            title="Acceleration")
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  model Tank "Pressurised propellant tank"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    parameter Real p_supply = 3.0e6 "Regulated supply pressure (Pa)";
    parameter Real m_eps = 5.0 "Empty-cutoff smoothing band (kg)";

    Real m(start = m_initial, fixed = true, min = 0) "Propellant mass (kg)";
    Real fuel_fraction "Remaining propellant fraction (0..1)";

    FluidPort_a port "Fluid outlet"
      annotation(Placement(transformation(extent={{-10,-90},{10,-70}})));
    MassSignalOutput mass_out "Current mass (kg)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
    Modelica.Blocks.Interfaces.RealOutput availability "Supply availability [0..1]"
      annotation(Placement(transformation(extent={{100,40},{120,60}})));
  equation
    port.p = p_supply;
    // Smooth saturation — differentiable everywhere so BDF doesn't
    // stall at the empty boundary.
    availability = m / (m + m_eps);
    // RHS form keeps the alias visible to rumoca's published signals.
    fuel_fraction = m / m_initial;
    der(m) = port.m_flow;
    mass_out = m;
    assert(m >= -1e-3, "Tank ran dry — availability gating failed.");
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
        Rectangle(extent=DynamicSelect(
            {{-40,40},{40,-70}},
            {{-40, -70 + 110 * (m / m_initial)}, {40, -70}}),
          lineColor={50,80,140},
          fillColor={120,160,220},
          fillPattern=FillPattern.Solid),
        Text(extent={{-46,58},{46,48}},
          textString="Propellant",
          textColor={0,0,80}),
        Text(extent={{-46,48},{46,37}},
          textString=DynamicSelect("kg", String(m) + " kg"),
          textColor={0,0,80}),
        Text(extent={{-120,-85},{120,-110}},
          textString="%name",
          textColor={40,40,40})
      }));
  end Tank;

  model Valve "Opening-controlled throttle valve"
    parameter Real m_flow_max(unit = "kg/s") = 20 "Mass flow at full opening";

    Modelica.Blocks.Interfaces.RealInput opening(min = 0, max = 100, unit = "%")
      "Valve opening [0..100 %]"
      annotation(Placement(transformation(extent={{-20,80},{20,120}})));
    FluidPort_a port_a "Inlet (supplier side)"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    FluidPort_b port_b "Outlet (consumer side)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
    Modelica.Blocks.Interfaces.RealInput availability(min = 0, max = 1) = 1.0
      "Upstream availability [0..1]"
      annotation(Placement(transformation(extent={{-120,40},{-100,60}})));
  equation
    port_a.m_flow = (opening / 100) * m_flow_max * availability;
    port_a.m_flow + port_b.m_flow = 0;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        Polygon(points={{-60,40},{-60,-40},{0,0}},
          lineColor={40,40,40},
          fillColor={180,180,190},
          fillPattern=FillPattern.Solid),
        Polygon(points={{60,40},{60,-40},{0,0}},
          lineColor={40,40,40},
          fillColor={180,180,190},
          fillPattern=FillPattern.Solid),
        Line(points={{0,0},{0,80}}, color={60,60,60}, thickness=0.4),
        Rectangle(extent={{-15,80},{15,90}},
          lineColor={40,40,40},
          fillColor={200,200,210},
          fillPattern=FillPattern.Solid),
        Text(extent={{-120,-28},{120,-48}},
          // Live opening % via MLS §18 DynamicSelect.
          textString=DynamicSelect("Valve", "Valve " + String(opening) + " %"),
          textColor={40,40,40}),
        Text(extent={{-120,-52},{120,-77}},
          textString="%name",
          textColor={40,40,40})
      }));
  end Valve;

  model Engine "Liquid rocket engine — combustion chamber with throat"
    parameter Real Isp = 300 "Specific impulse (s)";
    parameter Real g0 = 9.81 "Standard gravity (m/s^2)";
    parameter Real p_chamber = 1.0e5 "Anchored chamber pressure (Pa)";

    FluidPort_b port "Propellant intake"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    ThrustForceOutput thrust "Thrust (N)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    port.p = p_chamber;
    thrust = Isp * g0 * port.m_flow;
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
        Text(extent={{-120,-105},{120,-130}},
          textString="%name",
          textColor={40,40,40})
      }));
  end Engine;

  model Airframe "Vehicle body — 1-D vertical flight dynamics"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";

    ThrustForceInput thrust_in "Thrust (N)"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    MassSignalInput mass_in "Current propellant mass (kg)"
      annotation(Placement(transformation(extent={{-120,40},{-100,60}})));

    Real altitude(start = 0, fixed = true) "m";
    Real velocity(start = 0, fixed = true) "m/s";
    Real total_mass "kg";
    // RHS form (not der(velocity)) — alias elimination would otherwise
    // drop this from the published signal stream.
    Real acceleration "m/s2";
  equation
    total_mass = dry_mass + mass_in;
    der(altitude) = velocity;
    acceleration = (thrust_in - total_mass * g) / total_mass;
    der(velocity) = acceleration;
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
        Text(extent={{-120,-95},{120,-120}},
          textString="%name",
          textColor={40,40,40})
      }));
  end Airframe;

end AnnotatedRocketStage;
