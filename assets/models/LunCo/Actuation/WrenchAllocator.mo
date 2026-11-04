within LunCo.Actuation;

model WrenchAllocator
  "Generic bounded six-degree-of-freedom actuator allocator"
  parameter Integer actuator_count(min = 1) = 1;
  parameter Integer allocation_iterations(min = 1) = 16
    "Cyclic coordinate sweeps for the bounded wrench solve";
  parameter Real wrench_matrix[6, actuator_count]
    "Maximum body-local wrench about the body origin per unit command";
  parameter Real allocation_relaxation(min = 0.0, max = 1.0) = 0.9
    "Bounded coordinate-descent relaxation";
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
  input Real center_of_mass_x(unit = "m") = 0.0
    "Wrench reference in body coordinates; zero explicitly selects the origin";
  input Real center_of_mass_y(unit = "m") = 0.0;
  input Real center_of_mass_z(unit = "m") = 0.0;

  output Real command[actuator_count];

  Real wrench_body[6];
  Real command_iteration[actuator_count, allocation_iterations + 1];
  Real wrench_about_com[6, actuator_count];
  Real coupling[actuator_count, actuator_count]
    "Live actuator Gram matrix about the authoritative COM";
  Real projected_wrench[actuator_count];
  Real column_norm_squared[actuator_count];

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
    for row in 1:3 loop
      wrench_about_com[row, i] = wrench_matrix[row, i];
    end for;
    wrench_about_com[4, i] = wrench_matrix[4, i]
      - center_of_mass_y * wrench_matrix[3, i] + center_of_mass_z * wrench_matrix[2, i];
    wrench_about_com[5, i] = wrench_matrix[5, i]
      - center_of_mass_z * wrench_matrix[1, i] + center_of_mass_x * wrench_matrix[3, i];
    wrench_about_com[6, i] = wrench_matrix[6, i]
      - center_of_mass_x * wrench_matrix[2, i] + center_of_mass_y * wrench_matrix[1, i];
    for j in 1:actuator_count loop
      coupling[i, j] = sum(wrench_about_com[row, i] * wrench_about_com[row, j]
        for row in 1:6);
    end for;
  end for;

  // Bounded coordinate descent consumes already updated commands in each sweep.
  // The live Gram matrix factors the gradient W'*(W*q-demand) into
  // coupling*q-W'*demand with the current physical wrench reference.
  for i in 1:actuator_count loop
    command_iteration[i, 1] = lower_command[i];
    column_norm_squared[i] = coupling[i, i];
    projected_wrench[i] = sum(wrench_about_com[row, i] * wrench_body[row]
      for row in 1:6);
  end for;
  for k in 1:allocation_iterations loop
    for i in 1:actuator_count loop
      command_iteration[i, k + 1] = if column_norm_squared[i] > 0.0 then
        max(lower_command[i], min(upper_command[i],
        command_iteration[i, k] - allocation_relaxation
          * (sum(coupling[i, j]
              * command_iteration[j, if j < i then k + 1 else k]
              for j in 1:actuator_count) - projected_wrench[i])
          / column_norm_squared[i])) else command_iteration[i, k];
    end for;
  end for;
  for i in 1:actuator_count loop
    command[i] = command_iteration[i, allocation_iterations + 1];
  end for;
end WrenchAllocator;
