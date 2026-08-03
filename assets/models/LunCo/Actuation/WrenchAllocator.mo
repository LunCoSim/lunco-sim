within LunCo.Actuation;

model WrenchAllocator
  "Generic six-degree-of-freedom actuator allocator"
  parameter Integer actuator_count(min = 1) = 1;
  parameter Real allocation_pinv[actuator_count, 6]
    "Projection-time pseudo-inverse of the authored actuator wrench matrix";
  parameter Real lower_command[actuator_count]
    "Lower command limit for every actuator";
  parameter Real upper_command[actuator_count]
    "Upper command limit for every actuator";

  input Real desired_force_x = 0.0;
  input Real desired_force_y = 0.0;
  input Real desired_force_z = 0.0;
  input Real desired_torque_x = 0.0;
  input Real desired_torque_y = 0.0;
  input Real desired_torque_z = 0.0;

  output Real command[actuator_count];

  Real wrench_body[6];
  Real raw_command[actuator_count];

equation
  // Every input and every actuator column is body-local. World transforms are
  // deliberately outside Modelica: Avian applies each authored actuator's
  // local direction and mount to the live rigid body.
  wrench_body[1] = desired_force_x;
  wrench_body[2] = desired_force_y;
  wrench_body[3] = desired_force_z;
  wrench_body[4] = desired_torque_x;
  wrench_body[5] = desired_torque_y;
  wrench_body[6] = desired_torque_z;

  for i in 1:actuator_count loop
    raw_command[i] = allocation_pinv[i, 1] * wrench_body[1]
      + allocation_pinv[i, 2] * wrench_body[2]
      + allocation_pinv[i, 3] * wrench_body[3]
      + allocation_pinv[i, 4] * wrench_body[4]
      + allocation_pinv[i, 5] * wrench_body[5]
      + allocation_pinv[i, 6] * wrench_body[6];
    command[i] = max(lower_command[i], min(upper_command[i], raw_command[i]));
  end for;
end WrenchAllocator;
