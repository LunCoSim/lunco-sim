class_name ModelicaConnector
extends ModelicaBase

enum ConnectorType {
    INSIDE,
    OUTSIDE,
    MECHANICAL,
    ELECTRICAL,
    THERMAL,
    FLUID,
    SIGNAL,
    NONE
}

var type: ConnectorType
var variables: Dictionary = {}  # name -> ModelicaVariable
var connection_sets: Array = [] # Groups of connected variables
var expandable: bool = false

func _init(p_name: String, p_type: ConnectorType = ConnectorType.INSIDE) -> void:
    var decl = Declaration.new(p_name)
    add_declaration(decl)
    type = p_type

func add_variable(name: String, kind: ModelicaVariable.VariableKind = ModelicaVariable.VariableKind.REGULAR) -> ModelicaVariable:
    var var_obj = ModelicaVariable.new(name, kind)
    variables[name] = var_obj
    return var_obj

func get_variable(name: String) -> ModelicaVariable:
    return variables.get(name)

func has_variable(name: String) -> bool:
    return variables.has(name)

func connect_to(other: ModelicaConnector) -> bool:
    # Create a new connection set or add to existing one
    var new_set = true
    for set in connection_sets:
        if set.has(self) or set.has(other):
            set.append(self)
            set.append(other)
            new_set = false
            break
    
    if new_set:
        connection_sets.append([self, other])
    
    # Generate connection equations for each matching variable pair
    for var_name in variables:
        if other.has_variable(var_name):
            var var1 = variables[var_name]
            var var2 = other.get_variable(var_name)
            
            if var1.is_flow_variable():
                # Sum of flow variables = 0
                # This will be handled by the equation system
                pass
            else:
                # Equality of potential variables
                if var1.value != var2.value:  # Compare values instead of objects
                    var1.set_value(var2.value)
    return true  # Return success

func is_inside() -> bool:
    return type == ConnectorType.INSIDE

func is_outside() -> bool:
    return type == ConnectorType.OUTSIDE

func set_expandable(is_expandable: bool) -> void:
    expandable = is_expandable

func _to_string() -> String:
    var decl = get_declaration(declarations.keys()[0])
    var result = "Connector %s:\n" % decl.name
    result += "  Type: %s\n" % ConnectorType.keys()[type]
    result += "  Expandable: %s\n" % str(expandable)
    result += "  Variables:\n"
    for var_name in variables:
        result += "    %s\n" % var_name
    return result 