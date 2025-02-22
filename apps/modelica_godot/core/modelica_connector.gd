class_name ModelicaConnector
extends RefCounted

enum Type {
    NONE,
    MECHANICAL,
    ELECTRICAL,
    THERMAL,
    FLUID,
    SIGNAL
}

enum Unit {
    NONE,
    METER,
    NEWTON,
    KILOGRAM,
    SECOND,
    AMPERE,
    KELVIN,
    PASCAL,
    WATT,
    VOLT
}

var type: Type = Type.NONE
var variables: Dictionary = {}
var units: Dictionary = {}

func _init(connector_type: Type = Type.NONE) -> void:
    type = connector_type
    variables = {}
    units = {}

func add_variable(name: String, value: float = 0.0, unit: Unit = Unit.NONE) -> void:
    variables[name] = {
        "value": value,
        "unit": unit
    }

func get_variable(name: String) -> Variant:
    if variables.has(name):
        return variables[name].value
    return null

func set_variable(name: String, value: float) -> void:
    if variables.has(name):
        variables[name].value = value

func get_unit(name: String) -> Unit:
    if variables.has(name):
        return variables[name].unit
    return Unit.NONE

func to_dict() -> Dictionary:
    return {
        "type": type,
        "variables": variables.duplicate(),
        "units": units.duplicate()
    }

func from_dict(data: Dictionary) -> void:
    type = data.get("type", Type.NONE)
    variables = data.get("variables", {}).duplicate()
    units = data.get("units", {}).duplicate() 