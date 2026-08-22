within LunCo.Actuation;

model WrenchAllocator
  "Generic bounded six-degree-of-freedom actuator allocator"
  parameter Integer actuator_count(min = 1) = 1;
  parameter Integer allocation_iterations(min = 1) = 16
    "Projected-gradient iterations for the bounded wrench solve";
  parameter Real wrench_matrix[6, actuator_count]
    "Maximum six-component body wrench produced by one unit command";
  parameter Real allocation_step = 1.0
    "Stable projected-gradient step computed from authored wrench geometry";
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
  Real modelled_wrench[6, allocation_iterations];
  Real residual[6, allocation_iterations];
  Real gradient[actuator_count, allocation_iterations];
  Real command_iteration[actuator_count, allocation_iterations + 1];

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

  // Force actuators are one-sided physical devices: a command can open a
  // nozzle, but it cannot ask that nozzle for negative thrust. A plain
  // pseudo-inverse followed by a clamp solves a different problem and loses
  // authority whenever it distributes a signed wrench across opposing
  // actuators before the clamp. Solve the bounded least-squares problem in the
  // model itself instead. The fixed iteration count is deterministic and the
  // step is supplied by projection from the authored wrench matrix, so this
  // remains a reusable model rather than an RCS-specific branch.
  for i in 1:actuator_count loop
    command_iteration[i, 1] = lower_command[i];
  end for;

  for k in 1:allocation_iterations loop
    for row in 1:6 loop
      modelled_wrench[row, k] = sum(
        wrench_matrix[row, i] * command_iteration[i, k]
        for i in 1:actuator_count);
      residual[row, k] = modelled_wrench[row, k] - wrench_body[row];
    end for;
    for i in 1:actuator_count loop
      gradient[i, k] = sum(
        wrench_matrix[row, i] * residual[row, k]
        for row in 1:6);
      command_iteration[i, k + 1] = max(lower_command[i], min(
        upper_command[i], command_iteration[i, k]
          - allocation_step * gradient[i, k]));
    end for;
  end for;

  for i in 1:actuator_count loop
    command[i] = command_iteration[i, allocation_iterations + 1];
  end for;
end WrenchAllocator;
