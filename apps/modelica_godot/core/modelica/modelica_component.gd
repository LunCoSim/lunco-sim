class_name ModelicaComponent
extends ModelicaBase

var variables: Dictionary = {}      # name -> ModelicaVariable
var parameters: Dictionary = {}     # name -> ModelicaVariable
var parameter_metadata: Dictionary = {}  # Store additional parameter info
var connectors: Dictionary = {}     # name -> ModelicaConnector
var equations: Array = []          # List of equations
var binding_equations: Array = []   # Equations from declarations
var initial_equations: Array = []   # For initialization
var events: Array[Dictionary] = []  # Discrete events
var is_valid: bool = false

signal state_changed(variable_name: String, value: float)
signal parameter_changed(param_name: String, value: Variant)
signal event_triggered(event_name: String, data: Dictionary)

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
			if var_obj != null:  # Check if variable was created successfully
				var_obj.set_value(initial_value)
				# Also store in variables dictionary for easy access
				variables[name] = var_obj
				return var_obj
			return null
	
	# Regular variable
	var var_obj = ModelicaVariable.new(name, kind, initial_value)
	if var_obj != null:  # Check if variable was created successfully
		variables[name] = var_obj
		
		if kind == ModelicaVariable.VariableKind.PARAMETER:
			parameters[name] = var_obj
			# Initialize metadata if not exists
			if not parameter_metadata.has(name):
				parameter_metadata[name] = {
					"min": null,
					"max": null,
					"fixed": true,
					"evaluate": true,
					"description": ""
				}
		
		_validate_component()
		return var_obj
	return null

func add_state_variable(name: String, initial_value: float = 0.0) -> ModelicaVariable:
	var var_obj = add_variable(name, ModelicaVariable.VariableKind.STATE, initial_value)
	if var_obj != null:  # Check if variable was created successfully
		# Create corresponding derivative variable
		var der_name = "der(" + name + ")"
		var der_var = add_variable(der_name, ModelicaVariable.VariableKind.REGULAR, 0.0)
		if der_var != null:  # Check if derivative variable was created successfully
			der_var.set_derivative_of(name)
		return var_obj
	return null

func add_connector(name: String, type: ModelicaConnector.ConnectorType = ModelicaConnector.ConnectorType.INSIDE) -> ModelicaConnector:
	var conn = ModelicaConnector.new(name, type)
	connectors[name] = conn
	_validate_component()
	return conn

func add_equation(equation: String, is_initial: bool = false) -> void:
	if is_initial:
		initial_equations.append(equation)
	else:
		equations.append(equation)
	_validate_component()

func add_binding_equation(variable: String, expression: String) -> void:
	binding_equations.append({
		"variable": variable,
		"expression": expression
	})
	_validate_component()

func add_event(name: String, condition: String, action: String) -> void:
	events.append({
		"name": name,
		"condition": condition,
		"action": action,
		"is_active": false
	})
	_validate_component()

# Parameter metadata management
func set_parameter_min(name: String, min_value: float) -> void:
	if parameters.has(name):
		if not parameter_metadata.has(name):
			parameter_metadata[name] = {}
		parameter_metadata[name]["min"] = min_value
		_validate_component()

func set_parameter_max(name: String, max_value: float) -> void:
	if parameters.has(name):
		if not parameter_metadata.has(name):
			parameter_metadata[name] = {}
		parameter_metadata[name]["max"] = max_value
		_validate_component()

func set_parameter_fixed(name: String, fixed: bool) -> void:
	if parameters.has(name):
		if not parameter_metadata.has(name):
			parameter_metadata[name] = {}
		parameter_metadata[name]["fixed"] = fixed
		_validate_component()

func set_parameter_evaluate(name: String, evaluate: bool) -> void:
	if parameters.has(name):
		if not parameter_metadata.has(name):
			parameter_metadata[name] = {}
		parameter_metadata[name]["evaluate"] = evaluate
		_validate_component()

func set_parameter_description(name: String, description: String) -> void:
	if parameters.has(name):
		if not parameter_metadata.has(name):
			parameter_metadata[name] = {}
		parameter_metadata[name]["description"] = description

func get_parameter_metadata(name: String) -> Dictionary:
	if name in parameter_metadata:
		return parameter_metadata[name]
	return {}

func is_parameter_fixed(name: String) -> bool:
	if name in parameter_metadata:
		return parameter_metadata[name].get("fixed", true)
	return true

func is_parameter_evaluable(name: String) -> bool:
	if name in parameter_metadata:
		return parameter_metadata[name].get("evaluate", true)
	return true

func validate_parameter(name: String, value: Variant) -> bool:
	if not parameters.has(name):
		return false
		
	var param = parameters[name]
	if param == null:
		return false
		
	# Get metadata
	var metadata = get_parameter_metadata(name)
	
	# Check type
	if not (value is float or value is int or value is bool or value is String):
		return false
		
	# Check range for numeric types
	if value is float or value is int:
		var min_val = metadata.get("min")
		var max_val = metadata.get("max")
		
		if min_val != null and float(value) < min_val:
			return false
		if max_val != null and float(value) > max_val:
			return false
	
	return true

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
	if validate_parameter(name, value):
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

func _validate_component() -> void:
	# Basic validation - can be extended based on requirements
	is_valid = true
	# Add validation logic here as needed

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
		var metadata = get_parameter_metadata(param_name)
		result += "    %s" % param_name
		if metadata.get("description", "") != "":
			result += " (%s)" % metadata["description"]
		result += "\n"
	
	result += "  Connectors:\n"
	for conn_name in connectors:
		result += "    %s\n" % conn_name
	
	result += "  Events:\n"
	for event in events:
		result += "    %s: %s -> %s\n" % [event.name, event.condition, event.action]
	
	return result 
