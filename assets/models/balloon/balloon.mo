model Balloon
  parameter Real g = 9.81 "Gravity acceleration m/s²";
  // Note: balloon mass lives on the Avian RigidBody entity as `Mass`.
  // Modelica no longer subtracts weight from netForce — Avian's gravity
  // system applies `F = -m*g` as a separate force. Keep the Avian Mass
  // value in sync with `mass` here if you tune it.
  parameter Real mass = 4.5 "Reference balloon mass kg (matches Avian Mass)";
  // Max gas volume: slightly larger than sphere mesh (r=1m → V≈4.19 m³)
  parameter Real maxVolume = 6.0 "Maximum gas volume m³";
  parameter Real gasConstant = 287.058 "J/(kg·K) for air";
  // Standard sphere drag coefficient
  parameter Real dragCoeff = 0.47 "Sphere drag coefficient";
  // Slow thermal response — volume changes over ~3 s
  parameter Real tau = 3.0 "Volume thermal response time constant s";
  // Initial volume matches sphere mesh (r=1m → V≈4.19 m³)
  parameter Real initVolume = 4.0 "Initial gas volume m³";

  // Inputs from Avian physics (real-time feedback)
  input Real height = 0 "Altitude m from Avian position.y";
  input Real velocity = 0 "Vertical velocity m/s from Avian";

  // State variable (gives Modelica something to integrate)
  Real volume(start = initVolume) "Gas volume m³ with thermal lag";

  // Derived values (algebraic) — declared as outputs so rumoca preserves
  // them in the solver index instead of substituting them away.
  output Real temperature "Ambient temperature K (standard atmosphere)";
  output Real airDensity "Air density kg/m³";
  output Real buoyancy "Buoyancy force N = rho * V * g";
  output Real drag "Drag force N opposing motion";
  output Real netForce "External force N from balloon physics = buoyancy - drag (gravity applied by Avian)";

equation
  // Standard atmosphere (linear approximation, valid 0–11 km)
  temperature = 288.15 - 0.0065 * height;
  // Air density from ideal gas law with altitude pressure correction
  airDensity = (101325.0 / (gasConstant * temperature))
             * (1.0 - 0.0065 * height / 288.15) ^ 5.255;

  // Volume dynamics — thermal lag (first-order response)
  tau * der(volume) + volume = maxVolume * (temperature / 288.15);

  // Buoyancy (Archimedes' principle)
  buoyancy = airDensity * volume * g;

  // Drag: F = 0.5 * rho * Cd * A * v^2, cross-section A = pi * r^2
  // Sphere radius from volume: r = cbrt(3*V / (4*pi))
  // Using volume^(2/3) as proxy for A (proportional to r^2).
  // Sign: drag opposes velocity direction.
  drag = 0.5 * airDensity * dragCoeff * (3.14159 * volume ^ (2.0 / 3.0))
         * velocity * abs(velocity);

  // Net external force routed to Avian. Gravity (weight) is applied by
  // Avian's gravity system separately — we export only the aerodynamic
  // contribution (lift minus drag).
  netForce = buoyancy - drag;
end Balloon;
