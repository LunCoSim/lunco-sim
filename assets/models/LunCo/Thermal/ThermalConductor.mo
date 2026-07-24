within LunCo.Thermal;
// Linear thermal conduction between two heat ports: Q = G * (port_a.T - port_b.T)
model ThermalConductor
  parameter Real G = 10.0 "Thermal conductance, W/K";

  HeatPort port_a;
  HeatPort port_b;
equation
  port_a.Q = G * (port_a.T - port_b.T);
  port_b.Q = -port_a.Q;
end ThermalConductor;
