within LunCo.Thermal;
// Vacuum radiative cooling (Stefan-Boltzmann T^4 law into deep space sink).
model Radiator
  extends LunCo.Icons.Radiator;
  parameter Real area = 1.0 "Radiator emitting area, m²";
  parameter Real emissivity = 0.90 "Thermal emissivity, 0..1";
  parameter Real sigma = 5.670374e-8 "Stefan-Boltzmann constant, W/(m².K⁴)";
  parameter Real T_sink = 3.0 "Deep space background sink temperature, K";

  HeatPort port;
  Real Q_rad "Radiated thermal power, W";
  output Real temperature_k(unit="K") "Radiator surface temperature";
  output Real heat_rejection_w(unit="W") "Thermal power rejected to deep space";
equation
  Q_rad = emissivity * area * sigma * (port.T^4 - T_sink^4);
  port.Q = max(0.0, Q_rad);
  temperature_k = port.T;
  heat_rejection_w = port.Q;
end Radiator;
