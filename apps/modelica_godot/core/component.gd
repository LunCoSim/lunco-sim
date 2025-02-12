class_name ModelicaComponent
extends Node

# Component state
var component_name: String = ""
var connectors: Dictionary = {}
var parameters: Dictionary = {}
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

func _init(comp_name: String = "", desc: String = ""):
	component_name = comp_name
	description = desc

func add_connector(name: String, type: ModelicaConnector.Type) -> void:
	if name not in connectors:
		connectors[name] = ModelicaConnector.new(type)
		_validate_component()

func add_parameter(name: String, value: float, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	parameters[name] = value
	_validate_component()

func add_variable(name: String, initial_value: float = 0.0, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	variables[name] = initial_value
	_validate_component()

func add_state_variable(name: String, initial_value: float = 0.0, unit: ModelicaConnector.Unit = ModelicaConnector.Unit.NONE) -> void:
	state_variables[name] = initial_value
	variables[name] = initial_value  # Also add to regular variables for easy access
	# Initialize corresponding derivative variable
	der_variables["der(" + name + ")"] = 0.0
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
	if name in parameters:
		return parameters[name]
	push_error("Parameter not found: " + name)
	return 0.0

func get_variable(name: String) -> float:
	if name in variables:
		return variables[name]
	elif name in state_variables:
		return state_variables[name]
	elif name in der_variables:
		return der_variables[name]
	push_error("Variable not found: " + name)
	return 0.0

func set_variable(name: String, value: float) -> void:
	if name in variables:
		variables[name] = value
		emit_signal("state_changed", name, value)
	elif name in state_variables:
		state_variables[name] = value
		variables[name] = value  # Keep regular variables in sync
		emit_signal("state_changed", name, value)
	elif name in der_variables:
		der_variables[name] = value
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
	is_valid = true  # Simple validation for now

func to_dict() -> Dictionary:
	return {
		"name": component_name,
		"description": description,
		"type": "component",
		"connectors": _serialize_connectors(),
		"parameters": parameters,
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
	for param_name in data.get("parameters", {}):
		var param = data.get("parameters", {})[param_name]
		if param is Dictionary and param.has("value"):
			parameters[param_name] = param.duplicate()
	
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
