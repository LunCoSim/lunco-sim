within LunCo.Pointing;
import LunCo.Electrical.Pin;

// Reaction Control Wheel (Reaction Wheel / Flywheel for spacecraft attitude control).
// Stores angular momentum h = I_wheel * omega and draws power from EPS bus Pin during acceleration.
model ReactionWheel
  parameter Real i_wheel = 0.02 "Flywheel moment of inertia, kg.m²";
  parameter Real max_torque_nm = 0.2 "Maximum motor torque, N.m";
  parameter Real p_idle_w = 0.5 "Standby electronics power draw, W";
  parameter Real eta_motor = 0.85 "Motor efficiency, 0..1";

  input Real torque_cmd "Commanded control torque, N.m";

  Real omega(start = 0.0) "Flywheel spin rate, rad/s";
  output Real momentum_nms "Stored angular momentum, N.m.s";
  output Real torque_out_nm "Reaction torque applied to vehicle, N.m";
  output Real power_draw_w "Electrical power draw, W";

  Pin p "Electrical bus pin";
equation
  torque_out_nm = max(-max_torque_nm, min(max_torque_nm, torque_cmd));
  der(omega) = torque_out_nm / i_wheel;
  momentum_nms = i_wheel * omega;
  power_draw_w = p_idle_w + abs(torque_out_nm * omega) / max(0.1, eta_motor);
  p.i = power_draw_w / max(1.0, p.v);
end ReactionWheel;
