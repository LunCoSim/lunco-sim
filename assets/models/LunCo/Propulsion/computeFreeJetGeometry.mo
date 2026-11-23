within LunCo.Propulsion;

function computeFreeJetGeometry
  "Compute fixed free-jet station radii and supersonic Mach numbers"
  input Real exit_radius_m;
  input Real exit_mach;
  input Real gas_gamma;
  input Real observation_length_m;
  input Real exit_lip_angle_rad;
  output Real geometry[10];
protected
  constant Real pi = 3.141592653589793;
  Real area_exit;
  Real area_mach_exit;
  Real radius;
  Real area;
  Real z;
algorithm
  assert(exit_radius_m > 0.0 and exit_mach > 1.0,
    "FreeJetGeometry requires a positive exit radius and supersonic Mach number");
  assert(gas_gamma > 1.0 and gas_gamma < 2.0 and observation_length_m > 0.0,
    "FreeJetGeometry requires gamma in (1,2) and a positive observation horizon");
  assert(exit_lip_angle_rad >= 0.0 and exit_lip_angle_rad < 0.5,
    "FreeJetGeometry lip angle must be in [0,0.5) radians");
  area_exit := pi*exit_radius_m^2;
  area_mach_exit := (2.0/(gas_gamma+1.0)*(1.0+(gas_gamma-1.0)*exit_mach^2/2.0))
    ^((gas_gamma+1.0)/(2.0*(gas_gamma-1.0)))/exit_mach;
  for i in 1:5 loop
    z := observation_length_m*((i-1)/4.0)^2;
    radius := sqrt(exit_radius_m^2+2.0*(z/(sqrt(gas_gamma)*exit_mach))^2)
      + z*tan(exit_lip_angle_rad);
    area := pi*radius^2;
    geometry[i] := radius;
    geometry[5+i] := supersonicExitMach(area/area_exit*area_mach_exit,gas_gamma);
  end for;
end computeFreeJetGeometry;
