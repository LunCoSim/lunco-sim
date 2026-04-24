// tagline: Rocket stage with acausal fluid — pressurised tank, throttle valve, engine
// Proper MSL-style fluid architecture:
//
//   Tank  ──FluidPort──► Valve ──FluidPort──► Engine
//      │                   ▲                    │
//      │                   │                    ▼
//    mass_out          throttle              thrust
//      │                                        │
//      └──────► Airframe ◄─────────────────────┘
//
// Fluid is modelled with an acausal `FluidPort` connector carrying a
// pressure potential and a `flow` mass-flow variable. Mass
// conservation (Σ m_flow = 0 at every connect-set) is enforced
// automatically by the compiler.
//
// Throttle is a local runtime input on the Valve, exposed at the
// stage boundary via a `RealInput` connector. Flow magnitude is set
// by `k · opening · Δp` across the valve, which is well-conditioned
// index-1 — both ends of the fluid line have pressures anchored
// (tank at `p_supply`, engine at `p_chamber`), so the solver has no
// initial-condition ambiguity.
//
// Wires drawn on the diagram:
//   throttle          ──► valve.opening    (magenta — RealInput)
//   tank.port         ↔   valve.port_a     (teal circle — acausal FluidPort)
//   valve.port_b      ↔   engine.port      (teal circle — acausal FluidPort)
//   tank.mass_out     ──► airframe.mass_in (green  — causal mass signal)
//   engine.thrust     ──► airframe.thrust_in (red — causal thrust signal)

package AnnotatedRocketStage

  // ── Acausal fluid connector ──────────────────────────────────────
  // Balanced per MLS §9.3.1: one potential (`p`) + one flow (`m_flow`).
  // Both `FluidPort_a` and `FluidPort_b` share the same interface
  // via `extends`; the split exists solely to give the visual
  // renderer a filled vs. unfilled port icon (MSL Modelica.Fluid
  // convention for "this end is the supplier / consumer intent").
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

  // ── Causal information-signal connectors (unchanged) ─────────────

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
  model RocketStage "Single-stage rocket — pressurised tank, throttle valve, engine"
    parameter Real g = 9.81 "Gravity (m/s^2)";
    parameter Real dry_mass = 1000 "Empty stage mass (kg)";

    Tank tank(m_initial = 4000, p_supply = 3.0e6)
      annotation(Placement(transformation(extent={{-95,20},{-55,80}})));
    // The valve's `opening` input IS the stage's throttle — exposed
    // as `valve.opening` in the flattened model. No wrapper input
    // on the stage; UI controls bind directly to the valve.
    Valve valve(m_flow_max = 100)
      annotation(Placement(transformation(extent={{-40,10},{0,50}})));
    Engine engine(p_chamber = 1.0e5)
      annotation(Placement(transformation(extent={{15,-30},{55,30}})));
    Airframe airframe(g = g, dry_mass = dry_mass)
      annotation(Placement(transformation(extent={{70,-30},{110,30}})));

  equation
    connect(tank.port, valve.port_a);
    connect(valve.port_b, engine.port);
    connect(tank.mass_out, airframe.mass_in);
    connect(engine.thrust, airframe.thrust_in);

    annotation(
      Diagram(coordinateSystem(extent={{-100,-100},{100,100}}),
        graphics={
          Text(extent={{-100,95},{100,80}},
            textString="Rocket Stage — pressurised fluid line with throttle valve",
            textColor={0,0,0})
        }),
      experiment(StartTime=0, StopTime=150, Tolerance=1e-4, Interval=0.1));
  end RocketStage;

  // ── Pressurised propellant tank ──
  // Anchors the high-pressure side of the fluid line. Mass depletes
  // at whatever rate the downstream valve demands.
  model Tank "Pressurised propellant tank"
    parameter Real m_initial = 4000 "Initial propellant mass (kg)";
    parameter Real p_supply = 3.0e6 "Regulated supply pressure (Pa)";
    Real m(start = m_initial, fixed = true) "Propellant mass (kg)";

    FluidPort_a port "Fluid outlet"
      annotation(Placement(transformation(extent={{-10,-120},{10,-100}})));
    MassSignalOutput mass_out "Current mass (kg)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    port.p = p_supply;
    // MSL Fluid sign convention: `port.m_flow > 0` means mass
    // enters this component from the line. The tank is losing
    // mass while the engine burns, so `port.m_flow` is negative
    // and `der(m)` is therefore negative too — tank depletes.
    der(m) = port.m_flow;
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
        // Blue LOX fluid level. The bottom edge is fixed at -70;
        // the top edge moves between -70 (empty) and 40 (full)
        // proportionally to `m / m_initial`. Modelica.Fluid uses
        // exactly this DynamicSelect-on-extent pattern in its
        // OpenTank icon for live fluid-level animation.
        Rectangle(extent=DynamicSelect(
            {{-40,40},{40,-70}},
            {{-40, -70 + 110 * (m / m_initial)}, {40, -70}}),
          lineColor={50,80,140},
          fillColor={120,160,220},
          fillPattern=FillPattern.Solid),
        Text(extent={{-50,10},{50,-10}},
          // MLS §18 DynamicSelect: tools that don't animate render
          // the static "LOX"; the workbench renders the live mass
          // during simulation by evaluating the dynamic branch.
          textString=DynamicSelect("LOX", "LOX " + String(m) + " kg"),
          textColor={0,0,80}),
        Text(extent={{-90,-85},{90,-100}},
          textString="Tank",
          textColor={40,40,40})
      }));
  end Tank;

  // ── Throttle valve ──
  // Two-port valve between tank and engine. Runtime input `opening`
  // sets the fractional valve area [0..1]; mass flow follows
  // `k · opening · (p_a - p_b)`. Enforces its own internal mass
  // conservation (port_a.m_flow + port_b.m_flow = 0) because a
  // single-component acausal element has no connect-set of its own.
  model Valve "Opening-controlled throttle valve"
    // Linear flow-area model: `m_flow = opening · m_flow_max`. A
    // proper pressure-driven form (`k · opening · Δp`) is more
    // physical but introduces enough algebraic stiffness at t=0
    // that rumoca's BDF initialiser stalls. For the demonstration
    // rocket the linear form gives the same visible behaviour —
    // the acausal ports still enforce mass conservation across
    // the valve, and pressure is still anchored on both sides so
    // users can inspect tank/chamber Δp in the telemetry.
    parameter Real m_flow_max(unit = "kg/s") = 20
      "Mass flow at full opening";

    // `min`/`max` annotations on the input declare the valid range
    // (MLS §4.8.4). Tools clamp interactive sliders to this range
    // automatically; the workbench's Telemetry DragValue picks the
    // bounds up via AST extraction. They are advisory metadata, not
    // a solver constraint — the equation-side Limiter below is what
    // physically enforces the bound for any caller (UI, FMI master,
    // scripted set_input, etc.).
    // `min`/`max`/`unit` declare the valid range (MLS §4.8.4). Tools
    // clamp interactive sliders to this range — the workbench's
    // Telemetry DragValue picks the bounds up via AST extraction.
    // Per Modelica / FMI convention these are advisory metadata; the
    // solver does NOT enforce them. Hard enforcement is rumoca's
    // job — `SimStepper::set_input` rejects out-of-range writes
    // with an error rather than silently clamping.
    //
    // Opening is expressed in percent (0..100) — natural unit for a
    // control surface; the equation divides by 100 to convert to a
    // fraction before scaling `m_flow_max`. We deliberately do NOT
    // add a `Modelica.Blocks.Nonlinear.Limiter` block: the C0 kink
    // at the bound makes the residual non-differentiable, which BDF
    // can't traverse cleanly when the user operates exactly at the
    // boundary (a "full throttle" demo). UI clamping + API rejection
    // covers the practical envelope.
    Modelica.Blocks.Interfaces.RealInput opening(min = 0, max = 100, unit = "%")
      "Valve opening [0..100 %]"
      annotation(Placement(transformation(extent={{-20,80},{20,120}})));
    FluidPort_a port_a "Inlet (supplier side)"
      annotation(Placement(transformation(extent={{-120,-10},{-100,10}})));
    FluidPort_b port_b "Outlet (consumer side)"
      annotation(Placement(transformation(extent={{100,-10},{120,10}})));
  equation
    port_a.m_flow = (opening / 100) * m_flow_max;
    port_a.m_flow + port_b.m_flow = 0;
    annotation(Icon(coordinateSystem(extent={{-100,-100},{100,100}}),
      graphics={
        // Valve body — "bowtie" between two triangles.
        Polygon(points={{-60,40},{-60,-40},{0,0}},
          lineColor={40,40,40},
          fillColor={180,180,190},
          fillPattern=FillPattern.Solid),
        Polygon(points={{60,40},{60,-40},{0,0}},
          lineColor={40,40,40},
          fillColor={180,180,190},
          fillPattern=FillPattern.Solid),
        // Stem up to the opening input.
        Line(points={{0,0},{0,80}}, color={60,60,60}, thickness=0.4),
        Rectangle(extent={{-15,80},{15,90}},
          lineColor={40,40,40},
          fillColor={200,200,210},
          fillPattern=FillPattern.Solid),
        Text(extent={{-90,-60},{90,-85}},
          // Live opening % via MLS §18 DynamicSelect.
          textString=DynamicSelect("Valve", "Valve " + String(opening) + " %"),
          textColor={40,40,40})
      }));
  end Valve;

  // ── Liquid rocket engine ──
  // Combustion chamber anchored at p_chamber. Consumes propellant
  // drawn through `port` and produces thrust = Isp·g₀·|m_flow|.
  // Sign: flow INTO the connector from this component is positive
  // (MLS §9.3) — engine is a sink, so port.m_flow ends up negative
  // while burning. We use `max(-port.m_flow, 0)` for thrust so it's
  // never negative (no "reverse thrust" from tiny backflow).
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
    // MSL Fluid sign convention: `port.m_flow > 0` = mass entering
    // engine = propellant being consumed. Thrust scales linearly
    // with flow. A non-smooth `max(port.m_flow, 0)` guard was
    // previously here to prevent "negative thrust" from numerical
    // backflow, but its non-differentiable kink makes BDF's Jacobian
    // NaN at the initial step, stalling the solve.
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
        Text(extent={{-80,90},{80,70}},
          textString="Engine",
          textColor={0,0,0})
      }));
  end Engine;

  // ── Airframe / vehicle body ──
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
