within LunCo.Thermal;
// Ideal external coolant reservoir with finite heat-rejection capacity.
// This boundary exports heat; it does not model pump electrical consumption.
model LimitedCoolingSink
  parameter Real maximum_heat_rejection_w(unit="W") = 100.0;
  parameter Real minimum_temperature_k(unit="K") = 273.15;
  parameter Real conductance_w_per_k(unit="W/K") = 10.0;
  HeatPort port;
  output Real heat_rejection_w(unit="W");
  output Real temperature_k(unit="K");
equation
  assert(maximum_heat_rejection_w >= 0.0 and minimum_temperature_k > 0.0
    and conductance_w_per_k > 0.0,
    "LimitedCoolingSink requires nonnegative capacity, positive absolute minimum temperature and conductance");
  heat_rejection_w = min(maximum_heat_rejection_w,
    conductance_w_per_k * max(0.0, port.T - minimum_temperature_k));
  port.Q = heat_rejection_w;
  temperature_k = port.T;
end LimitedCoolingSink;
