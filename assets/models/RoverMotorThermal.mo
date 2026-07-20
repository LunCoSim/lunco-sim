// Per-side drive motor thermal model: drive commands in, motor temperatures out.
//
// The OPTIONAL thermal model (`thermal = "basic"` variant on six_wheel_rover,
// the exemplar): a first-order lumped heat balance per side. Each side's motor
// bank dissipates `heat_w` at full command (I^2R plus gear losses, folded into
// one number) and sheds heat radiatively + conductively at `k_loss` W per
// kelvin above the environment. The equilibrium temperature at sustained full
// command is t_env + heat_w/k_loss = 250 + 72 = 322 K; the thermal mass `c_th`
// sets how long that takes (tau = c_th/k_loss = 1600 s — motors heat over
// half an hour of driving, the way real ones do, not over a spirited minute).
//
// PURE TELEMETRY: the outputs are readable ports (`temp_left`/`temp_right`,
// kelvin) — watchable, plottable, scriptable — and nothing in the sim acts on
// them. No rhai bridge; the whole component is this model plus its wiring.
// `t_env = 250 K` is a lunar-day ambient placeholder until a real environment
// bus exists.
//
// RUMOCA RULES (same as RoverDrivetrain.mo / LegStrut.mo): branch-free
// equations — `der(x) = expr` with `max`/`min` clamps only, no `if`/`when`.
// Compiled by rumoca via `lunco:program:sourceAsset`; the drive inputs wire
// natively via `inputs:x.connect` to the rover's FSW drive ports.

model RoverMotorThermal
  parameter Real heat_w = 180.0 "Dissipation at full command, per side (W)";
  parameter Real k_loss = 2.5 "Radiative + conductive loss (W/K)";
  parameter Real t_env = 250.0 "Environment sink temperature (K) — lunar-day placeholder";
  parameter Real c_th = 4000.0 "Lumped thermal mass of one side's motors (J/K)";

  input Real drive_left "Normalized left-side drive command, -1..1";
  input Real drive_right "Normalized right-side drive command, -1..1";

  // Per-side temperature states. start = 250.0 is t_env spelled literally —
  // the states begin in equilibrium with the environment.
  Real tl(start = 250.0) "Left motor bank temperature (K)";
  Real tr(start = 250.0) "Right motor bank temperature (K)";

  output Real temp_left "Left motor bank temperature (K)";
  output Real temp_right "Right motor bank temperature (K)";
equation
  // max(x,-x) = |x| without a branch: heating follows command MAGNITUDE
  // (reverse works the motors just as hard), losses follow the excess over
  // ambient — and cool the motors if they ever start below it.
  der(tl) = (heat_w * max(drive_left, -drive_left) - k_loss * (tl - t_env)) / c_th;
  der(tr) = (heat_w * max(drive_right, -drive_right) - k_loss * (tr - t_env)) / c_th;
  temp_left = tl;
  temp_right = tr;
end RoverMotorThermal;
