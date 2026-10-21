within LunCo.Thermal;
// Grey-body radiative exchange with an isothermal external environment.
model Radiator
  extends LunCo.Icons.Radiator;
  parameter Real area = 1.0 "Radiator emitting area, m²";
  parameter Real emissivity = 0.90 "Thermal emissivity, 0..1";
  parameter Real sigma = 5.670374e-8 "Stefan-Boltzmann constant, W/(m².K⁴)";
  parameter Real T_sink = 3.0 "Prescribed radiative environment temperature, K";

  HeatPort port;
  Real Q_rad "Signed outward radiative power, W";
  output Real temperature_k(unit="K") "Radiator surface temperature";
  output Real heat_rejection_w(unit="W") "Heat rejected to environment; negative heats surface";
equation
  assert(area >= 0.0 and emissivity >= 0.0 and emissivity <= 1.0
      and sigma > 0.0 and T_sink >= 0.0 and port.T >= 0.0,
    "Radiator requires nonnegative area/absolute temperatures, emissivity in [0,1] and positive sigma");
  Q_rad = emissivity * area * sigma * (port.T^4 - T_sink^4);
  // Signed exchange is essential: a hotter environment heats the surface.
  port.Q = Q_rad;
  temperature_k = port.T;
  heat_rejection_w = port.Q;
end Radiator;
