within LunCo.Actuation;

model RcsValveAllocator
  "Twelve-valve RCS allocator for a three-axis lander"
  parameter Real max_torque_nm(min = 1.0) = 6000.0;

  input Real desired_torque_x = 0.0;
  input Real desired_torque_y = 0.0;
  input Real desired_torque_z = 0.0;

  output Real pitch_pos_a_valve;
  output Real pitch_pos_b_valve;
  output Real pitch_neg_a_valve;
  output Real pitch_neg_b_valve;
  output Real roll_pos_a_valve;
  output Real roll_pos_b_valve;
  output Real roll_neg_a_valve;
  output Real roll_neg_b_valve;
  output Real yaw_pos_a_valve;
  output Real yaw_pos_b_valve;
  output Real yaw_neg_a_valve;
  output Real yaw_neg_b_valve;

protected
  Real pitch_pos;
  Real pitch_neg;
  Real roll_pos;
  Real roll_neg;
  Real yaw_pos;
  Real yaw_neg;

equation
  // Each opposed pair is a bounded torque demand.  The names describe the
  // authored nozzle groups, whose moment arms are part of the USD contract:
  //   pitch -> body X, roll -> body Z, yaw -> body Y.
  // A and B are the redundant nozzles on each physical torque axis, so they
  // receive the same command and the nozzle models remain responsible for
  // thrust, propellant and plume activity.
  pitch_pos = max(0.0, min(1.0, desired_torque_x / max_torque_nm));
  pitch_neg = max(0.0, min(1.0, -desired_torque_x / max_torque_nm));
  roll_pos = max(0.0, min(1.0, desired_torque_z / max_torque_nm));
  roll_neg = max(0.0, min(1.0, -desired_torque_z / max_torque_nm));
  yaw_pos = max(0.0, min(1.0, desired_torque_y / max_torque_nm));
  yaw_neg = max(0.0, min(1.0, -desired_torque_y / max_torque_nm));

  pitch_pos_a_valve = pitch_pos;
  pitch_pos_b_valve = pitch_pos;
  pitch_neg_a_valve = pitch_neg;
  pitch_neg_b_valve = pitch_neg;
  roll_pos_a_valve = roll_pos;
  roll_pos_b_valve = roll_pos;
  roll_neg_a_valve = roll_neg;
  roll_neg_b_valve = roll_neg;
  yaw_pos_a_valve = yaw_pos;
  yaw_pos_b_valve = yaw_pos;
  yaw_neg_a_valve = yaw_neg;
  yaw_neg_b_valve = yaw_neg;
end RcsValveAllocator;
