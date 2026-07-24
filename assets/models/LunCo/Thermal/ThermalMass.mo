within LunCo.Thermal;
// Lumped thermal capacitance (heat mass storage).
// Differential equation: c_th * der(port.T) = port.Q
model ThermalMass
  parameter Real c_th = 4000.0 "Thermal heat capacity, J/K";
  parameter Real T_init = 293.15 "Initial temperature at t=0, K";

  HeatPort port;
  Real T(start = T_init) "Component temperature, K";
  output Real temp_k "Temperature output for telemetry, K";
equation
  port.T = T;
  c_th * der(T) = port.Q;
  temp_k = T;
end ThermalMass;
