class_name ModelicaComponent
extends Node

# Component state
var connectors: Dictionary = {}
var parameters: Dictionary = {}
var variables: Dictionary = {}
var state_variables: Dictionary = {}  # Variables that have time derivatives
var der_variables: Dictionary = {}    # Derivatives of state variables
var equations: Array[String] = []
var events: Array[Dictionary] = []    # Discrete events
var is_valid: bool = false

# Component metadata
var component_name: String = ""
var description: String = ""

signal state_changed(variable_name: String, value: float)
signal event_triggered(event_name: String, data: Dictionary)

func _init(name: String = "", desc: String = ""):
	component_name = name
	description = desc

func add_connector(name: String, type: ModelicaConnector.Type) -> void:
	if name not in connectors:
		connectors[name] = ModelicaConnector.new(type)
		_validate_component()

func add_parameter(name: String, value: float, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	parameters[name] = {
		"value": value,
		"unit": unit
	}
	_validate_component()

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
	# Initialize corresponding derivative variable
	der_variables["der(" + name + ")"] = {
		"value": 0.0,
		"unit": unit  # Note: Should actually be unit/second, but keeping it simple for now
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

func get_parameter(name: String) -> float:
	var param = parameters.get(name)
	return param.value if param else 0.0

func get_parameter_unit(name: String) -> ModelicaConnector.Unit:
	var param = parameters.get(name)
	return param.unit if param else ModelicaConnector.Unit.NONE

func get_variable(name: String) -> float:
	var var_data = variables.get(name)
	if var_data:
		return var_data.value
	var_data = state_variables.get(name)
	if var_data:
		return var_data.value
	var_data = der_variables.get(name)
	return var_data.value if var_data else 0.0

func get_variable_unit(name: String) -> ModelicaConnector.Unit:
	var var_data = variables.get(name)
	if var_data:
		return var_data.unit
	var_data = state_variables.get(name)
	if var_data:
		return var_data.unit
	var_data = der_variables.get(name)
	return var_data.unit if var_data else ModelicaConnector.Unit.NONE

func set_variable(name: String, value: float) -> void:
	var var_data = variables.get(name)
	if var_data:
		var_data.value = value
		emit_signal("state_changed", name, value)
	var_data = state_variables.get(name)
	if var_data:
		var_data.value = value
		emit_signal("state_changed", name, value)
	var_data = der_variables.get(name)
	if var_data:
		var_data.value = value
		emit_signal("state_changed", name, value)

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
	
	# Check that all referenced variables in equations exist
	for equation in equations:
		var vars = _extract_variables_from_equation(equation)
		for var_name in vars:
			if not _variable_exists(var_name):
				is_valid = false
				push_error("Variable %s referenced in equation but not defined" % var_name)
	
	# Check that all state variables have corresponding derivative equations
	for state_var in state_variables:
		var der_name = "der(" + state_var + ")"
		if der_name not in der_variables:
			is_valid = false
			push_error("State variable %s has no derivative variable" % state_var)
	
	# Validate events
	for event in events:
		if not _validate_event(event):
			is_valid = false

func _extract_variables_from_equation(equation: String) -> Array[String]:
	# Placeholder - would need proper equation parser
	# Returns array of variable names referenced in equation
	return []

func _variable_exists(name: String) -> bool:
	return (name in variables 
			or name in state_variables 
			or name in der_variables 
			or name in parameters)

func _validate_event(event: Dictionary) -> bool:
	# Placeholder for event validation
	# Would check that condition and action are valid expressions
	return true

func to_dict() -> Dictionary:
	return {
		"name": component_name,
		"description": description,
		"connectors": _serialize_connectors(),
		"parameters": parameters,
		"variables": variables,
		"state_variables": state_variables,
		"der_variables": der_variables,
		"equations": equations,
		"events": events
	}

func from_dict(data: Dictionary) -> void:
	component_name = data.get("name", "")
	description = data.get("description", "")
	
	# Load connectors
	var connector_data = data.get("connectors", {})
	for connector_name in connector_data:
		var c_data = connector_data[connector_name]
		add_connector(connector_name, c_data.type)
		var connector = connectors[connector_name]
		for var_name in c_data.variables:
			connector.variables[var_name] = c_data.variables[var_name]
	
	# Load parameters
	parameters = data.get("parameters", {})
	
	# Load variables
	variables = data.get("variables", {})
	
	# Load state variables and their derivatives
	state_variables = data.get("state_variables", {})
	der_variables = data.get("der_variables", {})
	
	# Load equations
	equations = data.get("equations", [])
	
	# Load events
	events = data.get("events", [])
	
	# Validate the loaded component
	_validate_component()

func _serialize_connectors() -> Dictionary:
	var result = {}
	for connector_name in connectors:
		var connector = connectors[connector_name]
		result[connector_name] = {
			"type": connector.type,
			"variables": connector.variables,
			"units": connector.units
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
