within LunCo.Propulsion;

function computeFreeJetProfile
  "Reduced supersonic ideal-gas streamtube; five conservative stations"
  input Real mass_flow_kgs;
  input Real thrust_n;
  input Real exit_radius_m;
  input Real exit_mach;
  input Real gas_gamma;
  input Real gas_constant_j_kg_k;
  input Real ambient_pressure_pa;
  input Real observation_length_m;
  input Real exit_lip_angle_rad;
  input Real radiative_fraction;
  output Real values[35];
protected
  constant Real pi = 3.141592653589793;
  Real area_exit;
  Real speed_exit;
  Real temperature_exit;
  Real stagnation_temperature;
  Real total_specific_energy;
  Real pressure_exit;
  Real area_mach_exit;
  Real radius[5];
  Real density[5];
  Real temperature[5];
  Real speed[5];
  Real emission[5];
  Real station_mach;
  Real area;
  Real z;
  Real dz;
  Real dr;
  Real log_ratio;
  Real fraction;
  Real segment_integral;
  constant Real nodes[8] = {0.019855071751231856,0.10166676129318658,0.2372337950418355,0.40828267875217511,0.59171732124782483,0.7627662049581645,0.89833323870681348,0.9801449282487682};
  constant Real weights[8] = {0.050614268145188088,0.11119051722668723,0.15685332293894369,0.181341891689181,0.181341891689181,0.15685332293894369,0.11119051722668723,0.050614268145188088};
  Real raw_integral;
  Real jet_power;
  Real radiant_power;
  Real mass_residual;
  Real energy_residual;
  Real thrust_residual;
algorithm
  assert(exit_radius_m > 0.0 and exit_mach > 1.0,
    "FreeJetProfile requires a positive exit radius and supersonic Mach number");
  assert(gas_gamma > 1.0 and gas_gamma < 2.0 and gas_constant_j_kg_k > 0.0,
    "FreeJetProfile requires an ideal-gas gamma in (1,2) and positive gas constant");
  assert(observation_length_m > 0.0 and ambient_pressure_pa >= 0.0,
    "FreeJetProfile requires a positive observation horizon and nonnegative ambient pressure");
  assert(exit_lip_angle_rad >= 0.0 and exit_lip_angle_rad < 0.5,
    "FreeJetProfile lip angle must be in [0,0.5) radians");
  assert(radiative_fraction >= 0.0 and radiative_fraction <= 0.01,
    "FreeJetProfile passive radiation requires a fraction in [0,0.01]");
  assert(mass_flow_kgs >= 0.0 and thrust_n >= 0.0 and not (thrust_n > 0.0 and mass_flow_kgs <= 0.0),
    "FreeJetProfile requires nonnegative delivered flow/thrust and flow for positive thrust");
  area_exit := pi * exit_radius_m^2;
  // F = mdot*u_e + (p_e-p_a)*A_e; p_e = mdot*u_e/(gamma*M_e^2*A_e).
  // Effective Isp velocity includes pressure thrust and is not gas speed.
  speed_exit := if mass_flow_kgs > 0.0 and thrust_n > 0.0 then
    (thrust_n + ambient_pressure_pa*area_exit)
      / (mass_flow_kgs*(1.0 + 1.0/(gas_gamma*exit_mach^2))) else 0.0;
  temperature_exit := speed_exit^2/(gas_gamma*gas_constant_j_kg_k*exit_mach^2);
  stagnation_temperature := temperature_exit*(1.0 + (gas_gamma-1.0)*exit_mach^2/2.0);
  total_specific_energy := gas_gamma*gas_constant_j_kg_k/(gas_gamma-1.0)*temperature_exit
    + speed_exit^2/2.0;
  pressure_exit := if speed_exit > 0.0 then mass_flow_kgs*speed_exit
    /(area_exit*gas_gamma*exit_mach^2) else 0.0;
  assert(speed_exit <= 0.0 or pressure_exit >= ambient_pressure_pa,
    "FreeJetProfile supports underexpanded supersonic flow, not ambient shock compression");
  area_mach_exit := (2.0/(gas_gamma+1.0)*(1.0+(gas_gamma-1.0)*exit_mach^2/2.0))
    ^((gas_gamma+1.0)/(2.0*(gas_gamma-1.0)))/exit_mach;
  mass_residual := 0.0;
  energy_residual := 0.0;
  // Closure: exit transverse ideal-gas RMS spread, combined in quadrature
  // with the finite exit disk, plus the authored nozzle-lip direction. This
  // is an effective forward-streamtube closure, not a CFD/backflow solution.
  for i in 1:5 loop
    z := observation_length_m*((i-1)/4.0)^2;
    radius[i] := sqrt(exit_radius_m^2 + 2.0*(z/(sqrt(gas_gamma)*exit_mach))^2)
      + z*tan(exit_lip_angle_rad);
    area := pi*radius[i]^2;
    station_mach := supersonicExitMach(area/area_exit*area_mach_exit,gas_gamma);
    temperature[i] := stagnation_temperature/(1.0+(gas_gamma-1.0)*station_mach^2/2.0);
    speed[i] := station_mach*sqrt(gas_gamma*gas_constant_j_kg_k*temperature[i]);
    density[i] := if speed[i] > 0.0 then mass_flow_kgs/(area*speed[i]) else 0.0;
    emission[i] := density[i]^2;
    mass_residual := max(mass_residual,abs(density[i]*speed[i]*area
      - (if speed_exit > 0.0 then mass_flow_kgs else 0.0)));
    energy_residual := max(energy_residual,abs(mass_flow_kgs*(speed[i]^2/2.0
      + gas_gamma*gas_constant_j_kg_k/(gas_gamma-1.0)*temperature[i]
      - total_specific_energy)));
  end for;
  jet_power := mass_flow_kgs*total_specific_energy;
  radiant_power := radiative_fraction*jet_power;
  // Log-linear emissivity interpolation preserves the dilute positive field
  // without the bright axial bands of a linear rho-squared interpolant.
  // Eight-point Gauss-Legendre integrates that same shader field. Radiation
  // is passive postprocessing of the adiabatic gas solution (fraction <=1%).
  raw_integral := 0.0;
  for i in 1:4 loop
    dz := observation_length_m*(2*i-1)/16.0;
    dr := radius[i+1]-radius[i];
    segment_integral := 0.0;
    if emission[i] > 0.0 and emission[i+1] > 0.0 then
      log_ratio := log(emission[i+1]/emission[i]);
      for k in 1:8 loop
        fraction := nodes[k];
        segment_integral := segment_integral + weights[k]*emission[i]
          *exp(fraction*log_ratio)*(radius[i]+fraction*dr)^2;
      end for;
    end if;
    raw_integral := raw_integral + pi*dz*segment_integral;
  end for;
  for i in 1:5 loop
    emission[i] := if raw_integral > 0.0 then emission[i]*radiant_power/raw_integral else 0.0;
    values[i] := radius[i];
    values[5+i] := density[i];
    values[10+i] := temperature[i];
    values[15+i] := speed[i];
    values[20+i] := emission[i];
  end for;
  thrust_residual := if speed_exit > 0.0 then mass_flow_kgs*speed_exit
    +(pressure_exit-ambient_pressure_pa)*area_exit-thrust_n else 0.0;
  values[26] := jet_power;
  values[27] := radiant_power;
  values[28] := mass_residual;
  values[29] := energy_residual;
  values[30] := thrust_residual;
  values[31] := pressure_exit;
  values[32] := stagnation_temperature;
  values[33] := total_specific_energy;
  values[34] := speed_exit;
  values[35] := temperature_exit;
end computeFreeJetProfile;
