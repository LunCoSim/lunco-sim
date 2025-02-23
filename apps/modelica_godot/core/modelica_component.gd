class_name ModelicaComponent
extends ModelicaBase

var variables: Dictionary = {}      # name -> ModelicaVariable
var parameters: Dictionary = {}     # name -> ModelicaVariable
var connectors: Dictionary = {}     # name -> ModelicaConnector
var equations: Array = []          # List of equations
var binding_equations: Array = []   # Equations from declarations
var initial_equations: Array = []   # For initialization

signal state_changed(variable_name: String, value: float)
signal parameter_changed(param_name: String, value: Variant)

func _init(p_name: String, description: String = "") -> void:
    var decl = Declaration.new(p_name)
    decl.description = description
    add_declaration(decl)

func add_variable(name: String, kind: ModelicaVariable.VariableKind = ModelicaVariable.VariableKind.REGULAR, initial_value: float = 0.0) -> ModelicaVariable:
    if "." in name:
        # Handle port variables (e.g., port.position)
        var parts = name.split(".")
        if parts.size() == 2:
            var port_name = parts[0]
            var var_name = parts[1]
            
            # Create connector if it doesn't exist
            if not connectors.has(port_name):
                add_connector(port_name)
            
            # Add variable to connector
            var connector = connectors[port_name]
            var var_obj = connector.add_variable(var_name, kind)
            var_obj.set_value(initial_value)
            
            # Also store in variables dictionary for easy access
            variables[name] = var_obj
            return var_obj
    
    # Regular variable
    var var_obj = ModelicaVariable.new(name, kind, initial_value)
    variables[name] = var_obj
    
    if kind == ModelicaVariable.VariableKind.PARAMETER:
        parameters[name] = var_obj
    
    return var_obj

func add_state_variable(name: String, initial_value: float = 0.0) -> ModelicaVariable:
    var var_obj = add_variable(name, ModelicaVariable.VariableKind.STATE, initial_value)
    # Create corresponding derivative variable
    var der_name = "der(" + name + ")"
    var der_var = add_variable(der_name, ModelicaVariable.VariableKind.REGULAR, 0.0)
    der_var.set_derivative_of(name)
    return var_obj

func add_connector(name: String, type: ModelicaConnector.ConnectorType = ModelicaConnector.ConnectorType.INSIDE) -> ModelicaConnector:
    var conn = ModelicaConnector.new(name, type)
    connectors[name] = conn
    return conn

func add_equation(equation: String, is_initial: bool = false) -> void:
    if is_initial:
        initial_equations.append(equation)
    else:
        equations.append(equation)

func add_binding_equation(variable: String, expression: String) -> void:
    binding_equations.append({
        "variable": variable,
        "expression": expression
    })

func get_variable(name: String) -> ModelicaVariable:
    if "." in name:
        # Handle port variables
        var parts = name.split(".")
        if parts.size() == 2:
            var port_name = parts[0]
            var var_name = parts[1]
            if connectors.has(port_name):
                return connectors[port_name].get_variable(var_name)
    return variables.get(name)

func get_parameter(name: String) -> ModelicaVariable:
    return parameters.get(name)

func get_connector(name: String) -> ModelicaConnector:
    return connectors.get(name)

func set_variable_value(name: String, value: float) -> void:
    var var_obj = get_variable(name)
    if var_obj != null:
        if var_obj.set_value(value):
            emit_signal("state_changed", name, value)

func set_parameter_value(name: String, value: Variant) -> void:
    var param = get_parameter(name)
    if param != null:
        if param.set_value(value):
            emit_signal("parameter_changed", name, value)

func get_equations() -> Array:
    return equations

func get_initial_equations() -> Array:
    return initial_equations

func get_binding_equations() -> Array:
    return binding_equations

func _to_string() -> String:
    var decl = get_declaration(declarations.keys()[0])
    var result = "Component %s:\n" % decl.name
    if decl.description != "":
        result += "  Description: %s\n" % decl.description
    
    result += "  Variables:\n"
    for var_name in variables:
        result += "    %s\n" % var_name
    
    result += "  Parameters:\n"
    for param_name in parameters:
        result += "    %s\n" % param_name
    
    result += "  Connectors:\n"
    for conn_name in connectors:
        result += "    %s\n" % conn_name
    
    return result 