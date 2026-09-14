within LunCo.Actuation;

// Bounded allocator for paired one-sided force actuators.
//
// Each positive/negative pair is mounted symmetrically around a body axis.
// The allocator converts signed body torque requests into normalized valve
// commands; USD owns each actuator's force direction and mount, while this
// reusable model owns only the signed command split.  The duplicated A/B
// outputs let a vehicle distribute the requested moment across two physical
// nozzles without teaching the runtime about a particular airframe.
model SignedTorqueAllocator
  input Real desired_torque_x = 0.0 "Requested body torque X (N.m)";
  input Real desired_torque_y = 0.0 "Requested body torque Y (N.m)";
  input Real desired_torque_z = 0.0 "Requested body torque Z (N.m)";
  input Real max_torque_x = 4500.0 "Positive/negative X pair capacity (N.m)";
  input Real max_torque_y = 4500.0 "Positive/negative Y pair capacity (N.m)";
  input Real max_torque_z = 4500.0 "Positive/negative Z pair capacity (N.m)";

  output Real pitch_pos_a_valve "X moment pair actuator A";
  output Real pitch_pos_b_valve "X moment pair actuator B";
  output Real pitch_neg_a_valve "X moment pair actuator A";
  output Real pitch_neg_b_valve "X moment pair actuator B";
  output Real roll_pos_a_valve "Z moment pair actuator A";
  output Real roll_pos_b_valve "Z moment pair actuator B";
  output Real roll_neg_a_valve "Z moment pair actuator A";
  output Real roll_neg_b_valve "Z moment pair actuator B";
  output Real yaw_pos_a_valve "Positive Y moment actuator A";
  output Real yaw_pos_b_valve "Positive Y moment actuator B";
  output Real yaw_neg_a_valve "Negative Y moment actuator A";
  output Real yaw_neg_b_valve "Negative Y moment actuator B";

  Real x_positive;
  Real x_negative;
  Real y_positive;
  Real y_negative;
  Real z_positive;
  Real z_negative;

equation
  // One-sided force actuators cannot accept a signed command.  Split each
  // requested moment and clamp each half independently at its authored pair
  // capacity.  A zero capacity is treated as unavailable, never as an
  // unbounded actuator.
  x_positive = max(0.0, min(1.0,
    desired_torque_x / max(1.0e-9, max_torque_x)));
  x_negative = max(0.0, min(1.0,
    -desired_torque_x / max(1.0e-9, max_torque_x)));
  y_positive = max(0.0, min(1.0,
    desired_torque_y / max(1.0e-9, max_torque_y)));
  y_negative = max(0.0, min(1.0,
    -desired_torque_y / max(1.0e-9, max_torque_y)));
  z_positive = max(0.0, min(1.0,
    desired_torque_z / max(1.0e-9, max_torque_z)));
  z_negative = max(0.0, min(1.0,
    -desired_torque_z / max(1.0e-9, max_torque_z)));

  // The authored X-moment nozzles are mounted at opposite Z arms.  Their
  // local -Y thrust therefore gives the A pair a negative X moment and the B
  // pair a positive X moment.  Keep the normalized signed request aligned
  // with the physical wrench rather than with the historical port suffix.
  pitch_pos_a_valve = x_negative;
  pitch_pos_b_valve = x_positive;
  pitch_neg_a_valve = x_negative;
  pitch_neg_b_valve = x_positive;

  // The vehicle's Z-moment pair is cross-named by the historical USD port
  // identities.  The physical mount remains the source of truth; these
  // aliases make the sign explicit at the reusable allocator boundary.
  roll_pos_a_valve = z_negative;
  roll_pos_b_valve = z_positive;
  roll_neg_a_valve = z_positive;
  roll_neg_b_valve = z_negative;

  // Yaw is intentionally still a full signed pair, even when a particular
  // vehicle leaves it at zero or has no horizontal jet geometry.
  yaw_pos_a_valve = y_positive;
  yaw_pos_b_valve = y_positive;
  yaw_neg_a_valve = y_negative;
  yaw_neg_b_valve = y_negative;
end SignedTorqueAllocator;
