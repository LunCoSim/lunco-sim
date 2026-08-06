// tagline: Lander — local-frame flight-control law; physical actuation is composed in USD
model Lander
  "Powered-descent flight-control law. It converts pilot or guidance commands into a local thrust-valve request and a local attitude-torque request. It does not know the vehicle's world pose, engine thrust, propellant tank, nozzle layout, or actuator type."

  // Controller configuration is a live input surface. The USD airframe owns
  // the values, so Inspector edits change the active controller without
  // rewriting source or recompiling the Modelica component. Actuator limits
  // and propellant remain in the USD-composed propulsion networks.
  input Real spool_tau = 0.35
    "Human-stick command filter time constant (s)";
  input Real authority_filter_tau_s = 0.02
    "Attitude-authority filter time constant (s)";
  input Real minimum_time_constant_s = 1.0e-6
    "Smallest permitted controller time constant (s)";
  input Real authority_initial = 0.6
    "Initial filtered attitude authority";
  input Real command_lower_bound = 0.0
    "Lower bound for normalized valve and command inputs";
  input Real command_upper_bound = 1.0
    "Upper bound for normalized valve and command inputs";
  input Real touchdown_force_threshold_n = 250.0
    "Total leg load at the centre of the touchdown transition (N)";
  input Real touchdown_transition_width_n = 100.0
    "Width of the touchdown load transition (N)";
  // The rigid-body mass/COM/inertia names belong exclusively to the Avian endpoint on
  // the owning USD prim. Controller gains use a deliberately distinct input
  // surface so one connection cannot be claimed by the Modelica map backend
  // and accidentally prevent Avian from receiving live mass properties.
  input Real controller_inertia_xx = 6250.0;
  input Real controller_inertia_yy = 6250.0;
  input Real controller_inertia_zz = 6250.0;

  // Authority and command sources.
  input Real piloted = 0.0;
  input Real external_throttle = 0.0;
  input Real pitch = 0.0;
  input Real roll = 0.0;
  input Real yaw = 0.0;
  input Real guidance_throttle = 0.0;
  input Real guidance_pitch = 0.0;
  input Real guidance_roll = 0.0;
  input Real guidance_yaw = 0.0;
  input Real ang_authority = 0.6
    "Angular acceleration authority per unit attitude command";

  // Local sensor signals. The sensor/estimator owns frame conversion; this
  // controller sees only body-frame rates and body-frame attitude error.
  input Real gyro_x = 0.0 "Body-frame gyro rate about X (rad/s)";
  input Real gyro_y = 0.0 "Body-frame gyro rate about Y (rad/s)";
  input Real gyro_z = 0.0 "Body-frame gyro rate about Z (rad/s)";
  input Real attitude_error_x = 0.0 "Body-frame upright error about X";
  input Real attitude_error_y = 0.0 "Body-frame upright error about Y";
  input Real attitude_error_z = 0.0 "Body-frame upright error about Z";

  // Landing-load signals remain local vehicle facts. A dedicated touchdown
  // sensor can replace these inputs without changing the control contract.
  input Real leg_force_px = 0.0;
  input Real leg_force_nx = 0.0;
  input Real leg_force_pz = 0.0;
  input Real leg_force_nz = 0.0;

  input Real attitude_hold = 0.0
    "Enable local attitude stabilization";
  input Real hold_kp = 2.0
    "Attitude-error gain in angular acceleration units";
  input Real hold_kd = 2.5
    "Body-rate damping gain";
  input Real attitude_deadband_rad = 0.01
    "Attitude error below which the stabilizer requests no RCS torque";
  input Real rate_deadband_rad_s = 0.02
    "Body-rate below which the stabilizer requests no RCS torque";

  // Generic actuator demands. `throttle` is a normalized command for the
  // main-engine valve network. Torque is a body-frame request consumed by the
  // USD-composed attitude actuator network.
  output Real throttle "Main-engine valve-opening request, 0..1";
  output Real torque_x "Requested body torque about X (N.m)";
  output Real torque_y "Requested body torque about Y (N.m)";
  output Real torque_z "Requested body torque about Z (N.m)";
  output Real touchdown "Touchdown signal from local landing loads";

  Real filter_throttle(start = 0.0);
  Real filter_pitch(start = 0.0);
  Real filter_roll(start = 0.0);
  Real filter_yaw(start = 0.0);
  Real cmd_throttle;
  Real cmd_pitch;
  Real cmd_roll;
  Real cmd_yaw;
  Real live_authority;
  Real command_torque_x;
  Real command_torque_y;
  Real command_torque_z;
  Real hold_torque_x;
  Real hold_torque_y;
  Real hold_torque_z;
  Real hold_error_x;
  Real hold_error_y;
  Real hold_error_z;
  Real hold_rate_x;
  Real hold_rate_y;
  Real hold_rate_z;
  Real total_leg_force;

  LunCo.Logic.AboveThreshold touchdown_check;

initial equation
  // `authority_initial` is part of the live USD input surface, so it has
  // continuous variability and cannot legally appear as a variable's `start`
  // attribute. Sample it in the initialization system instead.
  live_authority = authority_initial;

equation
  // Keep scene-tunable values live for the runtime Modelica interface.
  der(live_authority) = (ang_authority - live_authority)
    / max(minimum_time_constant_s, authority_filter_tau_s);
  der(filter_throttle) = (external_throttle - filter_throttle)
    / max(minimum_time_constant_s, spool_tau);
  der(filter_pitch) = (pitch - filter_pitch)
    / max(minimum_time_constant_s, spool_tau);
  der(filter_roll) = (roll - filter_roll)
    / max(minimum_time_constant_s, spool_tau);
  der(filter_yaw) = (yaw - filter_yaw)
    / max(minimum_time_constant_s, spool_tau);

  // The possession flag selects the command source; it is not an attitude
  // authority gate. Guidance and pilot commands therefore use the same law.
  cmd_throttle = piloted * filter_throttle
    + (1.0 - piloted) * guidance_throttle;
  cmd_pitch = piloted * filter_pitch
    + (1.0 - piloted) * guidance_pitch;
  cmd_roll = piloted * filter_roll
    + (1.0 - piloted) * guidance_roll;
  cmd_yaw = piloted * filter_yaw
    + (1.0 - piloted) * guidance_yaw;

  throttle = max(command_lower_bound,
    min(command_upper_bound, cmd_throttle));
  command_torque_x = cmd_pitch * controller_inertia_xx * live_authority;
  command_torque_y = cmd_yaw * controller_inertia_yy * live_authority;
  command_torque_z = cmd_roll * controller_inertia_zz * live_authority;

  // Stabilization is expressed entirely in the body frame. The attitude
  // sensor emits the signed local error; the IMU emits local gyro rates.
  // RCS is a pulse actuator, not a constant trim motor.  The branch-free
  // dead-zone multiplier keeps small estimator noise from reopening a valve
  // after touchdown while preserving the signed error outside the dead zone.
  // `max` is intentional: the Modelica runtime reconstructs continuous
  // algebraic observables from branch-free expressions.
  hold_error_x = attitude_error_x * max(0.0,
    1.0 - attitude_deadband_rad / max(1.0e-9, abs(attitude_error_x)));
  hold_error_y = attitude_error_y * max(0.0,
    1.0 - attitude_deadband_rad / max(1.0e-9, abs(attitude_error_y)));
  hold_error_z = attitude_error_z * max(0.0,
    1.0 - attitude_deadband_rad / max(1.0e-9, abs(attitude_error_z)));
  hold_rate_x = gyro_x * max(0.0,
    1.0 - rate_deadband_rad_s / max(1.0e-9, abs(gyro_x)));
  hold_rate_y = gyro_y * max(0.0,
    1.0 - rate_deadband_rad_s / max(1.0e-9, abs(gyro_y)));
  hold_rate_z = gyro_z * max(0.0,
    1.0 - rate_deadband_rad_s / max(1.0e-9, abs(gyro_z)));

  hold_torque_x = attitude_hold * controller_inertia_xx
    * (hold_kp * hold_error_x - hold_kd * hold_rate_x);
  hold_torque_y = attitude_hold * controller_inertia_yy
    * (hold_kp * hold_error_y - hold_kd * hold_rate_y);
  hold_torque_z = attitude_hold * controller_inertia_zz
    * (hold_kp * hold_error_z - hold_kd * hold_rate_z);

  torque_x = command_torque_x + hold_torque_x;
  torque_y = command_torque_y + hold_torque_y;
  torque_z = command_torque_z + hold_torque_z;

  total_leg_force = leg_force_px + leg_force_nx + leg_force_pz + leg_force_nz;
  touchdown_check.threshold = touchdown_force_threshold_n;
  touchdown_check.transition_width = touchdown_transition_width_n;
  touchdown_check.minimum_transition_width = minimum_time_constant_s;
  touchdown_check.value = total_leg_force;
  touchdown = touchdown_check.active;
end Lander;
