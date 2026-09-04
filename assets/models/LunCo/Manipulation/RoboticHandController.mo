within LunCo.Manipulation;
model RoboticHandController "Three-finger hand command shaping and joint targets"
  extends LunCo.Icons.Mechanics;
  // Rhai writes the two semantic command ports. The controller owns the
  // continuous-time response and the symmetric finger allocation; it does not
  // know anything about USD paths or rigid-body implementation details.
  input Real open_command "Normalized open request, 0..1";
  input Real grasp_command "Normalized grasp request, 0..1";

  parameter Real tau = 0.25 "Command response time (s)";
  parameter Real left_close_angle = 0.9 "Left finger closing angle (rad)";
  parameter Real center_close_angle = 0.18 "Center finger support angle (rad)";
  parameter Real right_close_angle = -0.9 "Right finger closing angle (rad)";

  Real open_state(start = 1.0) "Filtered open command";
  Real grasp_state(start = 0.0) "Filtered grasp command";
  output Real closure "Effective closure fraction, 0..1";
  output Real left_angle "Left finger target angle (rad)";
  output Real center_angle "Center finger target angle (rad)";
  output Real right_angle "Right finger target angle (rad)";

equation
  // Branch-free bounded command states keep command transitions smooth while
  // retaining one authoritative law for every finger.
  der(open_state) = (max(0.0, min(1.0, open_command)) - open_state) / tau;
  der(grasp_state) = (max(0.0, min(1.0, grasp_command)) - grasp_state) / tau;
  closure = max(0.0, min(1.0, grasp_state * (1.0 - open_state)));
  left_angle = closure * left_close_angle;
  center_angle = closure * center_close_angle;
  right_angle = closure * right_close_angle;
end RoboticHandController;
