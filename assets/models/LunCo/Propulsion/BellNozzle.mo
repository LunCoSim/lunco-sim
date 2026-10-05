within LunCo.Propulsion;
model BellNozzle "A bell nozzle's geometry and what that geometry is worth."
  extends LunCo.Icons.Propulsion;
  constant Real pi = 3.141592653589793 "Circle constant";
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

  // Reduced ideal-gas assumptions. These are not a propellant-specific
  // equilibrium solution; vehicles must author their own gas properties.
  input Real gamma = 1.2 "Ratio of specific heats of the exhaust";
  input Real p_chamber = 5.5e6 "Chamber pressure (Pa)";
  input Real characteristic_velocity_mps = 1800.0
    "Authored combustion characteristic velocity (m/s), not derived from pressure";
  input Real p_ambient = 0.0 "Ambient pressure (Pa); 0 on the Moon";
  input Real g0 = 9.80665 "Standard gravity, for the Isp definition (m/s^2)";

  // ── Geometry ──────────────────────────────────────────────────────────────
  output Real throat_area "A_t (m^2)";
  output Real exit_area "A_e (m^2)";
  output Real exit_radius_m "Exit-plane radius exposed to connected consumers (m)";
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
  output Real exit_mach "Supersonic exit Mach number from the area ratio";
  output Real exit_pressure_ratio "Exit static pressure / chamber stagnation pressure";
  output Real cf "Thrust coefficient (-)";
  output Real c_star "Characteristic velocity (m/s)";
  output Real ideal_exhaust_velocity_mps
    "Effective ideal exhaust velocity including exit pressure thrust (m/s)";
  output Real isp_vac "Specific impulse at this design point (s)";
  output Real thrust "Thrust at chamber pressure (N)";
  output Real exit_pressure_pa "Exit-plane pressure exposed to connected consumers (Pa)";
  output Real chamber_pressure_pa
    "Chamber pressure exposed to connected consumers (Pa)";
  output Real ambient_pressure_pa
    "Ambient pressure exposed to connected consumers (Pa)";
protected
  function supersonicExitMach
    "Invert the isentropic area-Mach relation on its supersonic branch"
    input Real area_ratio;
    input Real gas_gamma;
    output Real mach;
  protected
    Real lower;
    Real upper;
    Real midpoint;
    Real midpoint_area;
  algorithm
    assert(gas_gamma > 1.0, "BellNozzle requires gamma > 1");
    assert(area_ratio >= 1.0, "BellNozzle exit area must be at least throat area");
    lower := 1.0;
    upper := 64.0;
    assert(area_ratio <= (2.0 / (gas_gamma + 1.0)
      * (1.0 + (gas_gamma - 1.0) / 2.0 * upper ^ 2))
      ^ ((gas_gamma + 1.0) / (2.0 * (gas_gamma - 1.0))) / upper,
      "BellNozzle area ratio exceeds the supersonic solve bracket");
    // Fixed 48 bisections give < 2.3e-13 Mach resolution on this bracket.
    // Explicit branch selection avoids convergence to the subsonic root.
    for iteration in 1:48 loop
      midpoint := (lower + upper) / 2.0;
      midpoint_area := (2.0 / (gas_gamma + 1.0)
        * (1.0 + (gas_gamma - 1.0) / 2.0 * midpoint ^ 2))
        ^ ((gas_gamma + 1.0) / (2.0 * (gas_gamma - 1.0))) / midpoint;
      if midpoint_area < area_ratio then
        lower := midpoint;
      else
        upper := midpoint;
      end if;
    end for;
    mach := (lower + upper) / 2.0;
  end supersonicExitMach;

equation
  throat_area = pi * throat_radius ^ 2;
  exit_area = pi * exit_radius ^ 2;
  exit_radius_m = exit_radius;
  expansion_ratio = exit_area / throat_area;

  r_station_1 = throat_radius + (exit_radius - throat_radius) * (1.0 / 3.0) ^ contour;
  r_station_2 = throat_radius + (exit_radius - throat_radius) * (2.0 / 3.0) ^ contour;

  // NASA Glenn isentropic area-Mach and pressure relations:
  // https://www.grc.nasa.gov/WWW/BGH/isentrop.html
  // Assumes a choked throat, ideal gas and attached supersonic expansion.
  // Separation, boundary-layer losses and combustion chemistry are not solved.
  exit_mach = supersonicExitMach(expansion_ratio, gamma);
  exit_pressure_ratio = (1.0 + (gamma - 1.0) / 2.0 * exit_mach ^ 2)
    ^ (-gamma / (gamma - 1.0));
  exit_pressure_pa = p_chamber * exit_pressure_ratio;
  // Compute from a dimensionless ratio so a cold chamber remains finite.
  // Ambient correction is omitted only at zero Pc, where design thrust is zero.
  cf = sqrt(2 * gamma ^ 2 / (gamma - 1)
            * (2 / (gamma + 1)) ^ ((gamma + 1) / (gamma - 1))
            * (1 - exit_pressure_ratio ^ ((gamma - 1) / gamma)))
       + (exit_pressure_ratio - (if p_chamber > 0.0 then p_ambient / p_chamber else 0.0))
         * expansion_ratio;

  c_star = characteristic_velocity_mps;
  ideal_exhaust_velocity_mps = cf * c_star;
  isp_vac = ideal_exhaust_velocity_mps / g0;
  thrust = cf * p_chamber * throat_area;
  chamber_pressure_pa = p_chamber;
  ambient_pressure_pa = p_ambient;
end BellNozzle;
