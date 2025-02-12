class_name ModelicaConnector
extends Resource

enum Type {
    MECHANICAL,
    ELECTRICAL,
    THERMAL,
    FLUID
}

enum Unit {
    NONE,
    METER,
    NEWTON,
    VOLT,
    AMPERE,
    KELVIN,
    WATT,
    PASCAL,
    KG_PER_SEC
}

var type: Type
var variables: Dictionary = {}
var units: Dictionary = {}
var is_connected: bool = false
var connected_to: Array[ModelicaConnector] = []

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
            units = {
                "position": Unit.METER,
                "force": Unit.NEWTON
            }
        Type.ELECTRICAL:
            variables = {
                "voltage": 0.0,   # Across variable
                "current": 0.0    # Through variable
            }
            units = {
                "voltage": Unit.VOLT,
                "current": Unit.AMPERE
            }
        Type.THERMAL:
            variables = {
                "temperature": 0.0,  # Across variable
                "heat_flow": 0.0     # Through variable
            }
            units = {
                "temperature": Unit.KELVIN,
                "heat_flow": Unit.WATT
            }
        Type.FLUID:
            variables = {
                "pressure": 0.0,     # Across variable
                "mass_flow": 0.0     # Through variable
            }
            units = {
                "pressure": Unit.PASCAL,
                "mass_flow": Unit.KG_PER_SEC
            }

func get_value(variable: String) -> float:
    return variables.get(variable, 0.0)

func set_value(variable: String, value: float) -> void:
    if variable in variables:
        variables[variable] = value

func can_connect_to(other: ModelicaConnector) -> bool:
    return type == other.type and not is_connected

func connect_to(other: ModelicaConnector) -> bool:
    if not can_connect_to(other):
        return false
        
    is_connected = true
    other.is_connected = true
    connected_to.append(other)
    other.connected_to.append(self)
    return true

func disconnect_from(other: ModelicaConnector) -> void:
    if other in connected_to:
        connected_to.erase(other)
        other.connected_to.erase(self)
        if connected_to.is_empty():
            is_connected = false
        if other.connected_to.is_empty():
            other.is_connected = false

func get_unit(variable: String) -> Unit:
    return units.get(variable, Unit.NONE)

func validate_connection_constraints() -> bool:
    if connected_to.is_empty():
        return true
        
    # Validate across variables are equal
    var across_vars = _get_across_variables()
    for var_name in across_vars:
        var value = get_value(var_name)
        for other in connected_to:
            if not is_equal_approx(value, other.get_value(var_name)):
                return false
                
    # Validate through variables sum to zero
    var through_vars = _get_through_variables()
    for var_name in through_vars:
        var sum = get_value(var_name)
        for other in connected_to:
            sum += other.get_value(var_name)
        if not is_equal_approx(sum, 0.0):
            return false
            
    return true

func _get_across_variables() -> Array[String]:
    match type:
        Type.MECHANICAL:
            return ["position"]
        Type.ELECTRICAL:
            return ["voltage"]
        Type.THERMAL:
            return ["temperature"]
        Type.FLUID:
            return ["pressure"]
    return []

func _get_through_variables() -> Array[String]:
    match type:
        Type.MECHANICAL:
            return ["force"]
        Type.ELECTRICAL:
            return ["current"]
        Type.THERMAL:
            return ["heat_flow"]
        Type.FLUID:
            return ["mass_flow"]
    return [] 