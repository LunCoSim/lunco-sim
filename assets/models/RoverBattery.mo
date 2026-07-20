// Rover traction battery: drive commands in, state-of-charge + alive gate out.
//
// The OPTIONAL battery model (`power = "battery"` variant on the skid/six-wheel
// rovers): an energy budget for the drivetrain. Consumption is REALISTIC in
// shape — an avionics floor (`idle_w`) plus per-side motor draw proportional to
// the commanded drive magnitude (`motor_w` at full command per side) — so a
// rover parked with the throttle centred sips power and one climbing at full
// command drains ~30x faster. `power = "infinite"` is simply the ABSENCE of
// this model; today's default behavior is untouched.
//
// `alive` is a smooth 0..1 cutoff: exactly 1.0 until the last ~2% of charge,
// then ramping to 0 as soc reaches empty. The rhai bridge
// (`assets/scenarios/rover_battery.rhai`) multiplies the drive ports by it, so
// a dying battery browns the motors out over the final stretch instead of
// stepping them dead. The SAME factor gates the drain, so soc glides into 0
// and can never integrate below it — a branch-free clamp, not an `if`.
//
// RUMOCA RULES (same as RoverDrivetrain.mo / LegStrut.mo): branch-free
// equations — `der(x) = expr` with `max`/`min` compositions only, no
// `if`/`when`. Compiled by rumoca via `info:sourceAsset`; the drive
// inputs wire natively via `inputs:x.connect` to the rover's FSW drive ports.

model RoverBattery
  parameter Real capacity_wh = 2000.0 "Pack capacity (Wh)";
  parameter Real motor_w = 250.0 "Per-side motor draw at full command (W)";
  parameter Real idle_w = 30.0 "Avionics floor — drawn even parked (W)";

  input Real drive_left "Normalized left-side drive command, -1..1";
  input Real drive_right "Normalized right-side drive command, -1..1";

  // State of charge, 1.0 = full. Declared as an internal state with the
  // outputs assigned from it, the same shape as RoverDrivetrain's tl/tr.
  Real q(start = 1.0) "State of charge fraction, 0..1";

  output Real soc "State of charge, 0..1";
  output Real alive "Smooth 0..1 low-charge cutoff — gates drive AND drain";
equation
  // max(x,-x) = |x| without a branch; the cutoff factor kills the drain as
  // soc -> 0 so the state settles at empty instead of going negative.
  // 3600 * capacity_wh converts Wh to J; drain is in fractions/second.
  der(q) = -(idle_w + motor_w * (max(drive_left, -drive_left) + max(drive_right, -drive_right)))
           * max(0.0, min(1.0, q * 50.0)) / (3600.0 * capacity_wh);
  soc = q;
  alive = max(0.0, min(1.0, q * 50.0));
end RoverBattery;
