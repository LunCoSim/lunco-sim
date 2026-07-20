within LunCo.Pointing;
model DishPattern "Parabolic dish main lobe: what pointing error costs the link."
  // The half of "pointing" that is not mechanism. A parabolic reflector's main
  // lobe is Gaussian to first order, so the usable fraction of peak gain falls
  // off with the SQUARE of the off-boresight angle measured in beamwidths.
  //
  // The beamwidth is not a magic number either: for a circular aperture the
  // half-power full angle is ≈ 70·λ/D in degrees, i.e. 1.22·λ/D in radians.
  // Author the dish's DIAMETER and the link's FREQUENCY in USD and the beam
  // follows — change the dish geometry and the comms behaviour changes with it.
  input Real point_error "angle between boresight and target (rad)";

  parameter Real diameter = 3.0 "reflector diameter (m)";
  parameter Real frequency = 2.2e9 "link frequency (Hz) — S-band by default";
  constant Real c = 2.99792458e8 "speed of light (m/s)";

  Real wavelength "carrier wavelength (m)";
  output Real beamwidth "half-power (-3 dB) full angle (rad)";
  output Real gain_frac "fraction of peak gain on the link, 0..1";
  output Real locked "1 while the target is inside the half-power beam";
equation
  wavelength = c / frequency;
  beamwidth = 1.22 * wavelength / diameter;
  // 4·ln2 = 2.7726 is the constant that puts gain_frac = 0.5 exactly at
  // ±beamwidth/2 — the definition of the half-power beamwidth.
  gain_frac = exp(-2.7726 * (point_error / beamwidth)^2);
  locked = if point_error < beamwidth / 2 then 1.0 else 0.0;
end DishPattern;
