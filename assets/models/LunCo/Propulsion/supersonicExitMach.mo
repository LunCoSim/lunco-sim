within LunCo.Propulsion;
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
