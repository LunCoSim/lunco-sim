within LunCo.Mechanics;
// An ideal reduction with a loss factor: the gearbox, stated as a relation between two
// flanges rather than as a pair of formulas someone applies by hand.
//
//   phi_a = ratio * phi_b          the output turns `ratio` times slower
//   tau_b = -ratio * eta * tau_a   and correspondingly harder, less what friction took
//
// `ratio` > 1 is a REDUCTION (motor fast, wheel slow), matching how every gearbox in this
// repo is authored (`lunco:gearbox:ratio = 1200`).
//
// Why this replaces arithmetic in Rust: `axle_peak_torque` and `axle_no_load_speed` are the
// two halves of the relation above, evaluated once at config time and thereafter carried as
// derived numbers. Stated as equations they cannot drift apart, and — the part no formula
// gives you — the SPEED path runs backwards through the same relation, so a motor composed
// behind this one sees the shaft speed the wheel imposes on it. That is what the causal
// `inputs:torque.connect` could never do, and what the torque-speed curve needs in order to
// leave Rust at all.
//
// Efficiency is applied on the torque path only: a gearbox loses force to friction, not
// revolutions. The authored output rating is a physical torque limit, not a runtime
// fallback or a Rust-side estimate.
model GearRatio
  extends LunCo.Icons.Mechanics;
  parameter Real ratio = 1200.0 "Reduction ratio, driving:driven (>1 reduces speed)";
  parameter Real eta = 0.85 "Mechanical efficiency, 0..1";
  parameter Real max_output_torque = 400.0 "Output-shaft torque rating, N.m";

  Flange flange_a "Fast side — the motor";
  Flange flange_b "Slow side — the wheel";
equation
  flange_a.phi = ratio * flange_b.phi;
  flange_b.tau = -max(-max_output_torque, min(max_output_torque, ratio * eta * flange_a.tau));
end GearRatio;
