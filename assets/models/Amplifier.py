# Cosim chain demo — Python amplifier.
#
# Reads `signal` (e.g. from a Modelica oscillator), multiplies by `gain`,
# writes to `scaled` so a downstream consumer (e.g. an Avian rigid body
# accepting `force_y`) can use it.
#
# Inputs / outputs are dicts pre-populated by `lunco-scripting::run_scripted_models`.
gain = 50.0
outputs["scaled"] = inputs.get("signal", 0.0) * gain
