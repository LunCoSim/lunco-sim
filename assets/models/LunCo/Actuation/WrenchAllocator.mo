within LunCo.Actuation;

model WrenchAllocator
  "Generic bounded six-degree-of-freedom actuator allocator"
  parameter Integer actuator_count(min = 1) = 1;
  parameter Integer allocation_iterations(min = 1) = 16
    "Cyclic coordinate sweeps for the bounded wrench solve";
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
  Real command_iteration[actuator_count, allocation_iterations + 1];
  final parameter Real coupling[actuator_count, actuator_count] =
    {{sum(wrench_matrix[row, i] * wrench_matrix[row, j] for row in 1:6)
      for j in 1:actuator_count} for i in 1:actuator_count}
    "Fixed actuator Gram matrix W-transpose times W";
  Real projected_wrench[actuator_count];
  Real column_norm_squared[actuator_count];
  Real matrix_norm_squared;
  Real relaxation;

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

  // Every coordinate uses the already updated commands earlier in its sweep.
  // This is bounded coordinate descent, rather than a simultaneous gradient
  // update that converges slowly for coupled translation/rotation geometry.
  // The fixed Gram matrix factors the gradient W'*(W*q-demand) into
  // coupling*q-W'*demand. This removes repeated six-axis residual equations
  // without changing sweep order, limits, relaxation or iteration count.
  for i in 1:actuator_count loop
    command_iteration[i, 1] = lower_command[i];
    column_norm_squared[i] = coupling[i, i];
    projected_wrench[i] = sum(wrench_matrix[row, i] * wrench_body[row]
      for row in 1:6);
  end for;
  matrix_norm_squared = sum(column_norm_squared[i] for i in 1:actuator_count);
  relaxation = max(0.0, min(1.0, allocation_step * matrix_norm_squared));
  for k in 1:allocation_iterations loop
    for i in 1:actuator_count loop
      command_iteration[i, k + 1] = max(lower_command[i], min(upper_command[i],
        command_iteration[i, k] - relaxation
          * (sum(coupling[i, j]
              * command_iteration[j, if j < i then k + 1 else k]
              for j in 1:actuator_count) - projected_wrench[i])
          / max(1.0e-12, column_norm_squared[i])));
    end for;
  end for;
  for i in 1:actuator_count loop
    command[i] = command_iteration[i, allocation_iterations + 1];
  end for;
end WrenchAllocator;
