class_name ModelicaVariable
extends ModelicaBase

enum VariableKind {
    REGULAR,
    PARAMETER,
    CONSTANT,
    STATE,
    FLOW,
    STREAM
}

enum Unit {
    NONE,
    METER,
    KILOGRAM,
    SECOND,
    AMPERE,
    KELVIN,
    MOLE,
    CANDELA,
    NEWTON,
    JOULE,
    WATT,
    PASCAL,
    VOLT,
    OHM,
    HERTZ
}

var kind: VariableKind
var value: Variant
var unit: Unit = Unit.NONE
var fixed: bool = false  # For initialization
var start: float = 0.0   # Initial value
var nominal: float = 1.0 # Nominal value
var min_value: float     # Optional minimum
var max_value: float     # Optional maximum
var derivative_of: String = ""  # For state variables, name of original variable

func _init(p_name: String, p_kind: VariableKind = VariableKind.REGULAR, p_value: Variant = 0.0) -> void:
    var decl = Declaration.new(p_name)
    add_declaration(decl)
    kind = p_kind
    value = p_value
    min_value = -INF
    max_value = INF

func is_state_variable() -> bool:
    return kind == VariableKind.STATE

func is_flow_variable() -> bool:
    return kind == VariableKind.FLOW

func is_parameter() -> bool:
    return kind == VariableKind.PARAMETER

func is_constant() -> bool:
    return kind == VariableKind.CONSTANT

func set_value(new_value: Variant) -> bool:
    if not _validate_value(new_value):
        push_error("Invalid value %s for variable %s" % [str(new_value), get_declaration(declarations.keys()[0]).name])
        return false
    value = new_value
    return true

func set_bounds(min_val: float, max_val: float) -> void:
    min_value = min_val
    max_value = max_val

func set_unit(p_unit: Unit) -> void:
    unit = p_unit

func set_derivative_of(var_name: String) -> void:
    derivative_of = var_name
    kind = VariableKind.STATE  # Automatically set as state variable

func _validate_value(val: Variant) -> bool:
    if not (val is float or val is int or val is bool):
        return false
    
    var float_val = float(val)
    if float_val < min_value or float_val > max_value:
        return false
    
    return true

func _to_string() -> String:
    var decl = get_declaration(declarations.keys()[0])
    var result = "Variable %s:\n" % decl.name
    result += "  Kind: %s\n" % VariableKind.keys()[kind]
    result += "  Value: %s\n" % str(value)
    if unit != Unit.NONE:
        result += "  Unit: %s\n" % Unit.keys()[unit]
    if is_state_variable() and derivative_of != "":
        result += "  Derivative of: %s\n" % derivative_of
    return result 