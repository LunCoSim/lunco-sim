within LunCo.Propulsion;

model RCSFlowAggregator
  "Sum RCS nozzle flow and split it across the bipropellant tanks"
  extends LunCo.Icons.Propulsion;

  parameter Real oxidizer_to_fuel_ratio = 2.6;
  parameter Real minimum_ratio = 1.0e-6;

  input Real flow_1 = 0.0;
  input Real flow_2 = 0.0;
  input Real flow_3 = 0.0;
  input Real flow_4 = 0.0;
  input Real flow_5 = 0.0;
  input Real flow_6 = 0.0;
  input Real flow_7 = 0.0;
  input Real flow_8 = 0.0;
  input Real flow_9 = 0.0;
  input Real flow_10 = 0.0;
  input Real flow_11 = 0.0;
  input Real flow_12 = 0.0;
  output Real total_flow_kgs;
  output Real fuel_flow_kgs;
  output Real oxidizer_flow_kgs;

equation
  total_flow_kgs = max(0.0, flow_1) + max(0.0, flow_2)
    + max(0.0, flow_3) + max(0.0, flow_4)
    + max(0.0, flow_5) + max(0.0, flow_6)
    + max(0.0, flow_7) + max(0.0, flow_8)
    + max(0.0, flow_9) + max(0.0, flow_10)
    + max(0.0, flow_11) + max(0.0, flow_12);
  fuel_flow_kgs = total_flow_kgs
    / (1.0 + max(minimum_ratio, oxidizer_to_fuel_ratio));
  oxidizer_flow_kgs = total_flow_kgs - fuel_flow_kgs;
end RCSFlowAggregator;
