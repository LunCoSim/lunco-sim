# Green Balloon Physics Logic — the Python twin of models/Balloon.mo.
# Inputs: height (m), velocity (m/s), rho0 (kg/m³), gravity (m/s²)
# Outputs: netForce (N)
#
# THE MEDIUM IS AN INPUT, NOT A CONSTANT. This used to bake in Earth's sea-level
# pressure and g = 9.81 while the scene it runs in is lunar (1.62 m/s²), which
# made buoyancy ~9.8x the body's weight with no altitude at which it stops. Its
# Modelica sibling had the identical defect and drove `RedBalloon` out of the
# world; the two must stay in step, so the same fix applies here.
#
# `rho0` defaults to 0 — vacuum — so an unstated medium produces no lift and no
# drag. A scene with an atmosphere authors it (1.225 for Earth at sea level),
# exactly as it does for the Modelica balloon.

maxVolume = 6.0
dragCoeff = 0.47
t0 = 288.15       # datum temperature, K
lapse = 0.0065    # temperature lapse rate, K/m

height = inputs.get("height", 0.0)
velocity = inputs.get("velocity", 0.0)
rho0 = inputs.get("rho0", 0.0)
gravity = inputs.get("gravity", 1.62)

# Profile temperature, floored at 1 K: the linear lapse would otherwise pass
# through zero at ~44 km and divide to infinity.
temperature = max(t0 - lapse * height, 1.0)

# Density from the datum value: rho = rho0 * (T0/T) * (pressure ratio). The
# pressure base is clamped at 0 so the fractional power stays real.
airDensity = rho0 * (t0 / temperature) * max(1.0 - lapse * height / t0, 0.0) ** 5.255

# Simple volume model (ignoring tau for now as we don't have persistent state in this stateless script execution)
volume = maxVolume * (temperature / t0)

# Buoyancy (Archimedes' principle) — at the SAME local gravity Avian applies
# to the body, so the two can never disagree.
buoyancy = airDensity * volume * gravity

# Drag: F = 0.5 * rho * Cd * A * v^2
# Sign: drag opposes velocity direction
area_proxy = 3.14159 * (volume ** (2.0 / 3.0))
drag = 0.5 * airDensity * dragCoeff * area_proxy * velocity * abs(velocity)

# Net external force routed to Avian. Gravity (weight) is applied by
# Avian's gravity system separately.
outputs["netForce"] = buoyancy - drag
outputs["airDensity"] = airDensity
outputs["volume"] = volume
outputs["temperature"] = temperature
