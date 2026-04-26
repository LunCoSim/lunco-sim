# Green Balloon Physics Logic
# Inputs: height (m), velocity (m/s)
# Outputs: netForce (N)

# Constants (matching balloon.mo)
g = 9.81
maxVolume = 6.0
dragCoeff = 0.47
gasConstant = 287.058
tau = 3.0 # s

height = inputs.get("height", 0.0)
velocity = inputs.get("velocity", 0.0)

# Standard atmosphere (linear approximation, valid 0–11 km)
temperature = 288.15 - 0.0065 * height

# Air density from ideal gas law with altitude pressure correction
airDensity = (101325.0 / (gasConstant * temperature)) * (1.0 - 0.0065 * height / 288.15) ** 5.255

# Simple volume model (ignoring tau for now as we don't have persistent state in this stateless script execution)
volume = maxVolume * (temperature / 288.15)

# Buoyancy (Archimedes' principle)
buoyancy = airDensity * volume * g

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
