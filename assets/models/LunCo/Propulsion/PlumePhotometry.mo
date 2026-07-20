within LunCo.Propulsion;
model PlumePhotometry "What an exhaust plume is worth as a light source."
  // Emissive geometry in a forward renderer illuminates nothing. A descent burn
  // therefore leaves the regolith directly under the vehicle lit only by the sun,
  // which on an airless body — hard shadows, no atmospheric scatter to hide it —
  // is the most obviously wrong thing in frame. So the plume needs a real light,
  // and this is the equation that says how bright it is.
  //
  //   USD  ── throttle, the plume's emitting geometry, its exitance ──►  this model
  //                                                                        │
  //                              luminous power (lm) and source radius  ◄───┘
  //
  // A glowing volume's luminous power IS its luminous exitance times its emitting
  // area, so that is what this computes — rather than a brightness authored by
  // hand, which has no connection to how bright the plume actually LOOKS and which
  // no change to the plume can ever reach.
  //
  // The shape parameters restate the plume shader's shape law
  // (`assets/shaders/plume.wgsl`), because the light and the drawn plume must be
  // the light and the plume OF one another. They are per-instance: the outer
  // shroud and the inner core are the same model with different numbers, and a
  // cone with no light simply mounts no photometry.

  input Real throttle = 0.0 "Engine throttle 0..1 — wired from the vessel";

  // ── The plume's emitting geometry, at full throttle ───────────────────────
  // The bounding cone the shader draws inside, in metres: `w_max` is its base
  // radius and `l_max` its length. Authored as the prim's own scale in USD, and
  // repeated here because a Modelica `input` can only be fed by a constant or by
  // a wire from another model's port, not by an attribute of the geometry.
  parameter Real w_max = 0.5 "Plume base radius at full throttle (m)";
  parameter Real l_max = 2.5 "Plume length at full throttle (m)";
  parameter Real width_idle = 0.28
    "Base-radius fraction at zero throttle; width blooms fast, then saturates";

  // ── Photometry ────────────────────────────────────────────────────────────
  // Rec.709 luma of the plume's authored colour. The standard luminance
  // weighting, and the reason a green flame of the same RGB magnitude lights a
  // scene far more than a blue one. The colour is LINEAR and un-normalised
  // (values above 1 are the whole point of an emissive), so this is a relative
  // radiance, not a 0..1 colour. KEEP IT AS `plume.wgsl`'s `core_color`:
  // (6.0, 3.5, 0.9) gives 0.2126*6.0 + 0.7152*3.5 + 0.0722*0.9 = 3.844.
  parameter Real luminance = 3.844 "Rec.709 luma of the plume's emissive colour";

  // The ONE authored photometric constant: luminous exitance per unit emissive
  // radiance, in lm/m^2. It is the unit conversion between "how bright the shader
  // says this surface is" and photometric output, which nothing in the scene can
  // supply — everything that VARIES is derived below.
  //
  // CALIBRATION. Bevy's `PointLight.intensity` is LUMINOUS POWER IN LUMENS ("the
  // amount of light emitted by this source in all directions"), not candela — so
  // illuminance at range d is phi/(4*pi*d^2), NOT phi/d^2. That factor of
  // 4*pi ~= 12.6 is easy to get backwards and presents as "the light is authored
  // but invisible". At full throttle on the core cone (`w_max` 0.5, `l_max` 2.5,
  // colour (6.0, 3.5, 0.9)): width 0.5, length 2.5, area ~= 4.005 m^2,
  // luminance ~= 3.844, so
  //   phi = 44200 * 3.844 * 4.005 ~= 680000 lm
  // and regolith ~3 m below the nozzle sees 680000/(4*pi*9) ~= 6000 lux against
  // this scene's 12000 lux sun — a clearly visible pool that reads as a second
  // source without flattening the scene. It falls off fast: ~540 lux at 10 m and
  // ~90 lux at the 25 m `lunco:light:range` cap, under 1% of the sun, so it
  // cannot quietly become scene fill. That 680000 figure is the empirically
  // verified one; a derivation landing far from it is wrong, not the target.
  parameter Real exitance = 44200.0
    "Luminous exitance per unit emissive radiance (lm/m^2)";

  // Bevy treats a point light's `radius` as a physical source SIZE, so growing it
  // with the plume softens the terminator of the light pool instead of leaving a
  // hard inverse-square dot.
  parameter Real r_idle = 0.06 "Source radius at zero throttle (m)";
  parameter Real r_gain = 0.6 "Additional source radius at full throttle (m)";

  output Real width "Plume base radius at this throttle (m)";
  output Real length "Plume length at this throttle (m)";
  output Real area "Lateral surface of the plume cone (m^2)";
  output Real intensity "Luminous power (lm) — Bevy PointLight.intensity";
  output Real radius "Physical source radius (m) — Bevy PointLight.radius";

  Real t "Throttle, saturated to 0..1";
equation
  t = min(1.0, max(0.0, throttle));

  // The same shape law the shader draws, so the light cannot describe a plume
  // other than the one on screen.
  width = (width_idle + (1.0 - width_idle) * t) * w_max;
  length = t * l_max;

  // The plume radiates from its FLANK, so the emitting surface is the cone's
  // lateral area — not its base, not its volume: A = pi * r * sqrt(r^2 + h^2).
  area = Modelica.Constants.pi * width * sqrt(width ^ 2 + length ^ 2);

  // Radiant output tracks chamber power, which for a throttled engine is very
  // nearly linear in mass flow. It comes out slightly superlinear here because
  // the emitting AREA also grows with throttle — the honest answer for a source
  // whose size is part of what you see.
  //
  // The endpoint is the property that matters: at t == 0 this is EXACTLY 0, not a
  // small residual. The lateral area never reaches zero (a zero-length cone still
  // has its base radius), so without this gate a dead engine would emit some
  // thousands of lumens and every coasting shot would pick up a phantom glow from
  // underneath.
  intensity = if t <= 0.0 then 0.0 else exitance * luminance * area;

  radius = r_idle + t * r_gain;
end PlumePhotometry;
