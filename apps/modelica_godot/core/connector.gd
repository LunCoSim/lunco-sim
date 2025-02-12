class_name ModelicaConnector
extends Resource

enum Type {
    MECHANICAL,
    ELECTRICAL,
    THERMAL,
    FLUID
}

var type: Type
var variables: Dictionary = {}

func _init(connector_type: Type):
    type = connector_type
    _setup_variables()

func _setup_variables():
    match type:
        Type.MECHANICAL:
            variables = {
                "position": 0.0,  # Across variable
                "force": 0.0      # Through variable
            }
        Type.ELECTRICAL:
            variables = {
                "voltage": 0.0,   # Across variable
                "current": 0.0    # Through variable
            }
        Type.THERMAL:
            variables = {
                "temperature": 0.0,  # Across variable
                "heat_flow": 0.0     # Through variable
            }
        Type.FLUID:
            variables = {
                "pressure": 0.0,     # Across variable
                "mass_flow": 0.0     # Through variable
            }

func get_value(variable: String) -> float:
    return variables.get(variable, 0.0)

func set_value(variable: String, value: float) -> void:
    if variable in variables:
        variables[variable] = value 