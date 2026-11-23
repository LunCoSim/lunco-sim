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
  Real calculated_area_ratio;
  Real area_residual;
  Real area_slope;
  Real candidate;
  Boolean converged;
algorithm
  assert(gas_gamma > 1.0, "BellNozzle requires gamma > 1");
  assert(area_ratio >= 1.0, "BellNozzle exit area must be at least throat area");
  lower := 1.0;
  upper := 64.0;
  assert(area_ratio <= (2.0 / (gas_gamma + 1.0)
    * (1.0 + (gas_gamma - 1.0) / 2.0 * upper ^ 2))
    ^ ((gas_gamma + 1.0) / (2.0 * (gas_gamma - 1.0))) / upper,
    "BellNozzle area ratio exceeds the supersonic solve bracket");
  if area_ratio == 1.0 then
    mach := 1.0;
  else
    // The near-sonic expansion is a useful initial estimate; the bracket
    // keeps Newton steps on the supersonic branch for the full input range.
    mach := min(upper, max(lower,
      1.0 + sqrt((gas_gamma + 1.0) / 2.0 * log(area_ratio))));
    converged := false;
    // Safeguarded Newton converges in a few iterations for plume stations.
    // The finite bracketed fallback preserves the area-Mach solution when a
    // Newton step leaves the supersonic root bracket.
    for iteration in 1:48 loop
      if not converged then
        calculated_area_ratio := (2.0 / (gas_gamma + 1.0)
          * (1.0 + (gas_gamma - 1.0) / 2.0 * mach ^ 2))
          ^ ((gas_gamma + 1.0) / (2.0 * (gas_gamma - 1.0))) / mach;
        area_residual := log(calculated_area_ratio / area_ratio);
        if abs(area_residual) <= 4.0e-15 then
          converged := true;
        else
          if area_residual < 0.0 then
            lower := mach;
          else
            upper := mach;
          end if;
          area_slope := 2.0 * (mach ^ 2 - 1.0)
            / (mach * (2.0 + (gas_gamma - 1.0) * mach ^ 2));
          candidate := mach - area_residual / area_slope;
          if candidate > lower and candidate < upper then
            if abs(candidate - mach) <= 1.0e-13 then
              mach := candidate;
              converged := true;
            else
              mach := candidate;
            end if;
          else
            midpoint := (lower + upper) / 2.0;
            if abs(midpoint - mach) <= 1.0e-13 then
              mach := midpoint;
              converged := true;
            else
              mach := midpoint;
            end if;
          end if;
        end if;
      end if;
    end for;
  end if;
end supersonicExitMach;
