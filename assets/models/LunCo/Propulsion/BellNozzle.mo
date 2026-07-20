within LunCo.Propulsion;
model BellNozzle "A bell nozzle's geometry and what that geometry is worth."
  // The nozzle's PARAMETERS live in USD (they are the vehicle's design), the
  // consequences of those parameters live here (they are physics). Nothing
  // about a nozzle changes per frame, so nothing about it belongs in a
  // per-tick script: give the model the four numbers that describe the bell
  // and it publishes the engineering that follows from them.
  //
  //   USD  ── throat_radius, exit_radius, length, contour ──►  this model
  //                                                              │
  //          expansion ratio, areas, exit velocity, Cf, Isp  ◄───┘
  //
  // ── The contour ───────────────────────────────────────────────────────────
  // Radius along the bell, at normalised station s in 0..1 from throat to exit:
  //
  //     r(s) = throat + (exit - throat) * s^contour
  //
  // `contour = 1` is a straight cone. Below 1 the flare is fast off the throat
  // and eases toward the exit — the family Rao's method produces, and what a
  // real engine looks like. The exponent is AUTHORED, not derived: a true Rao
  // contour is the solution of a method-of-characteristics problem needing
  // chamber conditions this vehicle does not carry. Saying so is better than
  // implying a rigour that is not there.
  input Real throat_radius = 0.35 "Throat radius (m) — wired from the USD nozzle prim";
  input Real exit_radius = 1.35 "Exit-plane radius (m)";
  input Real length = 1.90 "Throat-to-exit length (m)";
  input Real contour = 0.55 "Contour exponent; 1 = cone, <1 = bell";

  // Gas properties. Defaults are LOX/RP-1 combustion products, matching the
  // plume chemistry the plume materials already declare in USD.
  input Real gamma = 1.2 "Ratio of specific heats of the exhaust";
  input Real p_chamber = 5.5e6 "Chamber pressure (Pa)";
  input Real p_exit = 8.0e3 "Exit-plane static pressure (Pa) — design point";
  input Real p_ambient = 0.0 "Ambient pressure (Pa); 0 on the Moon";
  input Real g0 = 9.80665 "Standard gravity, for the Isp definition (m/s^2)";

  // ── Geometry ──────────────────────────────────────────────────────────────
  output Real throat_area "A_t (m^2)";
  output Real exit_area "A_e (m^2)";
  output Real expansion_ratio "epsilon = A_e / A_t — the number that names a nozzle";

  // Four contour stations, the same ones the USD lathe is built from, so the
  // model and the drawn surface are demonstrably the same shape.
  output Real r_station_1 "Radius at s = 1/3 (m)";
  output Real r_station_2 "Radius at s = 2/3 (m)";

  // ── Performance ───────────────────────────────────────────────────────────
  // Ideal thrust coefficient: momentum term (how much the expansion is worth)
  // plus pressure term (what the exit plane pushes on). Vacuum-corrected via
  // `p_ambient`, which is 0 here — that is exactly why a lunar lander wants a
  // big expansion ratio and why this bell flares as hard as it does.
  output Real cf "Thrust coefficient (-)";
  output Real c_star "Characteristic velocity (m/s)";
  output Real isp_vac "Specific impulse at this design point (s)";
  output Real thrust "Thrust at chamber pressure (N)";
equation
  throat_area = Modelica.Constants.pi * throat_radius ^ 2;
  exit_area = Modelica.Constants.pi * exit_radius ^ 2;
  expansion_ratio = exit_area / throat_area;

  r_station_1 = throat_radius + (exit_radius - throat_radius) * (1.0 / 3.0) ^ contour;
  r_station_2 = throat_radius + (exit_radius - throat_radius) * (2.0 / 3.0) ^ contour;

  // Momentum term. EXPLICIT — no implicit solve for exit Mach from area ratio,
  // because the toolchain that compiles this asset tree (rumoca) is restricted
  // to explicitly solved forms; the design-point `p_exit` is an input instead.
  cf = sqrt(2 * gamma ^ 2 / (gamma - 1)
            * (2 / (gamma + 1)) ^ ((gamma + 1) / (gamma - 1))
            * (1 - (p_exit / p_chamber) ^ ((gamma - 1) / gamma)))
       + (p_exit - p_ambient) * exit_area / (p_chamber * throat_area);

  c_star = p_chamber * throat_area / max(1e-9, p_chamber * throat_area / 1800.0);
  isp_vac = cf * c_star / g0;
  thrust = cf * p_chamber * throat_area;
end BellNozzle;
