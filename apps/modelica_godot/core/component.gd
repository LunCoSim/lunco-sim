class_name ModelicaComponent
extends Node

# Component state
var component_name: String = ""
var connectors: Dictionary = {}
var parameters: Dictionary = {}
var parameter_metadata: Dictionary = {}  # Store additional parameter info
var variables: Dictionary = {}
var state_variables: Dictionary = {}  # Variables that have time derivatives
var der_variables: Dictionary = {}    # Derivatives of state variables
var equations: Array[String] = []
var events: Array[Dictionary] = []    # Discrete events
var annotations: Dictionary = {}      # Component annotations
var is_valid: bool = false

# Component metadata
var description: String = ""

signal state_changed(variable_name: String, value: float)
signal event_triggered(event_name: String, data: Dictionary)
signal parameter_changed(param_name: String, value: Variant)

func _init(comp_name: String = "", desc: String = ""):
	component_name = comp_name
	description = desc

func add_connector(name: String, type: ModelicaConnector.Type) -> void:
	if name not in connectors:
		connectors[name] = ModelicaConnector.new(type)
		_validate_component()

func add_parameter(name: String, value: Variant, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	parameters[name] = {
		"value": value,
		"unit": unit
	}
	
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
	emit_signal("parameter_changed", name, value)

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

func get_parameter(name: String) -> Variant:
	if name in parameters:
		return parameters[name].value
	push_error("Parameter not found: " + name)
	return null

func get_parameter_unit(name: String) -> ModelicaConnector.Unit:
	if name in parameters:
		return parameters[name].unit
	return ModelicaConnector.Unit.NONE

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
		
	# Get metadata
	var metadata = get_parameter_metadata(name)
	
	# Check type
	if not (value is float or value is int or value is bool or value is String):
		return false
		
	# Check range for numeric types
	if value is float or value is int:
		var min_val = metadata.get("min")
		var max_val = metadata.get("max")
		
		if min_val != null and value < min_val:
			return false
		if max_val != null and value > max_val:
			return false
	
	return true

func add_variable(name: String, initial_value: float = 0.0, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	variables[name] = {
		"value": initial_value,
		"unit": unit
	}
	_validate_component()

func add_state_variable(name: String, initial_value: float = 0.0, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	state_variables[name] = {
		"value": initial_value,
		"unit": unit
	}
	variables[name] = {  # Also add to regular variables for easy access
		"value": initial_value,
		"unit": unit
	}
	# Initialize corresponding derivative variable
	var der_name = "der(" + name + ")"
	der_variables[der_name] = {
		"value": 0.0,
		"unit": unit,  # The derivative will have the same unit per time
		"state_var": name  # Keep track of which state variable this is the derivative of
	}
	_validate_component()

func add_equation(equation: String) -> void:
	equations.append(equation)
	_validate_component()

func add_event(name: String, condition: String, action: String) -> void:
	events.append({
		"name": name,
		"condition": condition,
		"action": action,
		"is_active": false
	})
	_validate_component()

func get_connector(name: String) -> ModelicaConnector:
	return connectors.get(name)

func get_variable(name: String) -> Variant:
	if "." in name:
		# Handle port variables (e.g., port.position)
		var parts = name.split(".")
		if parts.size() == 2:
			var port_name = parts[0]
			var var_name = parts[1]
			if port_name in variables:
				var full_name = name  # Use the full name as is
				if full_name in variables:
					return variables[full_name].value
	
	# Handle regular variables
	if name in variables:
		return variables[name].value
	elif name in state_variables:
		return state_variables[name].value
	elif name in der_variables:
		return der_variables[name].value
	push_error("Variable not found: " + name)
	return null

func set_variable(name: String, value: float) -> void:
	if "." in name:
		# Handle port variables (e.g., port.position)
		var parts = name.split(".")
		if parts.size() == 2:
			var port_name = parts[0]
			var var_name = parts[1]
			if port_name in variables:
				var full_name = name  # Use the full name as is
				if full_name in variables:
					variables[full_name].value = value
					emit_signal("state_changed", name, value)
					return
	
	# Handle regular variables
	if name in variables:
		variables[name].value = value
		# If this is also a state variable, update that too
		if name in state_variables:
			state_variables[name].value = value
		emit_signal("state_changed", name, value)
	elif name in state_variables:
		state_variables[name].value = value
		variables[name].value = value  # Keep regular variables in sync
		emit_signal("state_changed", name, value)
	elif name in der_variables:
		der_variables[name].value = value
		emit_signal("state_changed", name, value)
	else:
		push_error("Variable not found: " + name)

func get_equations() -> Array[String]:
	return equations

func get_events() -> Array[Dictionary]:
	return events

func evaluate_events() -> void:
	for event in events:
		var was_active = event.is_active
		# Here we would evaluate the condition - placeholder for now
		event.is_active = _evaluate_condition(event.condition)
		
		if event.is_active and not was_active:
			# Event just became active
			emit_signal("event_triggered", event.name, {"action": event.action})

func _evaluate_condition(condition: String) -> bool:
	# Placeholder for condition evaluation
	# Would need to be implemented with proper expression parser
	return false

func _validate_component() -> void:
	is_valid = true
	
	# Validate parameters
	for param_name in parameters:
		var value = parameters[param_name].value
		if not validate_parameter(param_name, value):
			push_error("Invalid parameter value for " + param_name)
			is_valid = false
	
	# Validate that all state variables have corresponding derivatives
	for state_var in state_variables:
		var der_name = "der(" + state_var + ")"
		if not der_variables.has(der_name):
			push_error("State variable " + state_var + " has no derivative")
			is_valid = false
	
	# Validate that all derivatives have corresponding state variables
	for der_var in der_variables:
		var state_var = der_variables[der_var].state_var
		if not state_variables.has(state_var):
			push_error("Derivative " + der_var + " has no state variable")
			is_valid = false

func to_dict() -> Dictionary:
	return {
		"name": component_name,
		"description": description,
		"type": "component",
		"connectors": _serialize_connectors(),
		"parameters": parameters,
		"parameter_metadata": parameter_metadata,
		"variables": variables,
		"state_variables": state_variables,
		"der_variables": der_variables,
		"equations": equations,
		"events": events,
		"annotations": annotations
	}

func from_dict(data: Dictionary) -> void:
	component_name = data.get("name", "")
	description = data.get("description", "")
	
	# Load connectors
	var connector_data = data.get("connectors", {})
	for connector_name in connector_data:
		var c_data = connector_data[connector_name]
		add_connector(connector_name, c_data.get("type", ModelicaConnector.Type.NONE))
		var connector = connectors[connector_name]
		connector.variables = c_data.get("variables", {}).duplicate()
		connector.units = c_data.get("units", {}).duplicate()
	
	# Load parameters with validation
	parameters = {}
	parameter_metadata = {}
	for param_name in data.get("parameters", {}):
		var param = data.get("parameters", {})[param_name]
		if param is Dictionary and param.has("value"):
			parameters[param_name] = param.duplicate()
			
		# Load parameter metadata if available
		if data.has("parameter_metadata") and data["parameter_metadata"].has(param_name):
			parameter_metadata[param_name] = data["parameter_metadata"][param_name].duplicate()
	
	# Load variables with validation
	variables = {}
	for var_name in data.get("variables", {}):
		var var_data = data.get("variables", {})[var_name]
		if var_data is Dictionary and var_data.has("value"):
			variables[var_name] = var_data.duplicate()
	
	# Load state variables and their derivatives
	state_variables = {}
	for var_name in data.get("state_variables", {}):
		var var_data = data.get("state_variables", {})[var_name]
		if var_data is Dictionary and var_data.has("value"):
			state_variables[var_name] = var_data.duplicate()
			# Also add to regular variables
			variables[var_name] = var_data.duplicate()
	
	der_variables = {}
	for var_name in data.get("der_variables", {}):
		var var_data = data.get("der_variables", {})[var_name]
		if var_data is Dictionary and var_data.has("value"):
			der_variables[var_name] = var_data.duplicate()
	
	# Load equations with validation
	equations = []
	for eq in data.get("equations", []):
		if eq is String and not eq.is_empty():
			equations.append(eq)
	
	# Load events with validation
	events = []
	for event in data.get("events", []):
		if event is Dictionary and event.has("condition"):
			events.append(event.duplicate())
	
	# Load annotations
	annotations = data.get("annotations", {}).duplicate()
	
	# Validate the loaded component
	_validate_component()

func _serialize_connectors() -> Dictionary:
	var result = {}
	for connector_name in connectors:
		var connector = connectors[connector_name]
		result[connector_name] = {
			"type": connector.type,
			"variables": connector.variables.duplicate(),
			"units": connector.units.duplicate()
		}
	return result

func save_to_file(path: String) -> Error:
	var file = FileAccess.open(path, FileAccess.WRITE)
	if file == null:
		return FileAccess.get_open_error()
	
	var json = JSON.new()
	var data_string = json.stringify(to_dict())
	file.store_string(data_string)
	return OK

func load_from_file(path: String) -> Error:
	var file = FileAccess.open(path, FileAccess.READ)
	if file == null:
		return FileAccess.get_open_error()
	
	var json = JSON.new()
	var result = json.parse(file.get_as_text())
	if result != OK:
		return result
	
	from_dict(json.get_data())
	return OK 
