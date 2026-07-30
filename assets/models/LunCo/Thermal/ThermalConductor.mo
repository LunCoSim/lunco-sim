within LunCo.Thermal;
// Linear thermal conduction between two heat ports: Q = G * (port_a.T - port_b.T)
model ThermalConductor
  extends LunCo.Icons.ThermalConductor;
  parameter Real G = 10.0 "Thermal conductance, W/K";

  HeatPort port_a;
  HeatPort port_b;
  output Real heat_flow_w(unit="W") "Heat flow from port A to port B; positive transfers A to B";
  output Real temperature_drop_k(unit="K") "Temperature at port A minus temperature at port B";
equation
  port_a.Q = G * (port_a.T - port_b.T);
  port_b.Q = -port_a.Q;
  heat_flow_w = port_a.Q;
  temperature_drop_k = port_a.T - port_b.T;
end ThermalConductor;
