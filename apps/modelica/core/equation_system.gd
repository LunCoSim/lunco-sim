class_name EquationSystem
extends RefCounted

# Collection of equations in the system
var equations = []

# Dictionary of variables with their metadata
# Each variable has:
# - value: current value
# - is_state: whether it's a state variable (has derivatives)
# - is_parameter: whether it's a parameter (constant)
# - metadata: additional information (e.g., description, units)
var variables = {}

# Current simulation time
var time: float = 0.0

# Initial values of variables (for reset)
var initial_values = {}

# Constructor
func _init():
	pass

# Add an equation to the system
func add_equation(equation) -> void:
	equations.append(equation)

# Add a variable with metadata
func add_variable(name: String, metadata: Dictionary = {}) -> void:
	if name in variables:
		push_warning("Variable already exists: " + name)
	
	# Default values
	var value = metadata.get("value", 0.0)
	var is_state = metadata.get("is_state", false)
	var is_parameter = metadata.get("is_parameter", false)
	
	variables[name] = {
		"value": value,
		"is_state": is_state,
		"is_parameter": is_parameter,
		"metadata": metadata
	}
	
	# Store initial value for reset
	initial_values[name] = value

# Get the value of a variable
func get_variable_value(name: String) -> float:
	if not name in variables:
		push_error("Variable not found: " + name)
		return 0.0
	
	return variables[name].value

# Set the value of a variable
func set_variable_value(name: String, value: float) -> void:
	if not name in variables:
		push_error("Cannot set value for non-existent variable: " + name)
		return
	
	variables[name].value = value

# Get current state of the system as a dictionary
func get_state() -> Dictionary:
	var state = {
		"time": time,
		"variables": {}
	}
	
	for name in variables:
		state.variables[name] = variables[name].value
	
	return state

# Set the state of the system from a dictionary
func set_state(state: Dictionary) -> void:
	if "time" in state:
		time = state.time
	
	if "variables" in state:
		for name in state.variables:
			if name in variables:
				variables[name].value = state.variables[name]
			else:
				push_warning("Unknown variable in state: " + name)

# Reset the system to initial values
func reset() -> void:
	time = 0.0
	
	for name in initial_values:
		if name in variables:
			variables[name].value = initial_values[name]

# Get variable dependencies (which equations depend on which variables)
func get_variable_dependencies() -> Dictionary:
	var dependencies = {}
	
	# For each equation, determine which variables it involves
	for i in range(equations.size()):
		var eq = equations[i]
		var vars = eq.get_involved_variables()
		
		for var_name in vars:
			if not var_name in dependencies:
				dependencies[var_name] = []
			dependencies[var_name].append(i)
	
	return dependencies

# Get all variables of a specific type
func get_variables_by_type(is_state: bool = false, is_parameter: bool = false) -> Array:
	var result = []
	
	for name in variables:
		var var_data = variables[name]
		if var_data.is_state == is_state and var_data.is_parameter == is_parameter:
			result.append(name)
	
	return result

# Get state variables
func get_state_variables() -> Array:
	return get_variables_by_type(true, false)

# Get parameter variables
func get_parameters() -> Array:
	return get_variables_by_type(false, true)

# Get algebraic variables (neither state nor parameter)
func get_algebraic_variables() -> Array:
	return get_variables_by_type(false, false)

# Check if the system has algebraic loops
func has_algebraic_loops() -> bool:
	# This is a simplified check - a more thorough implementation would use Tarjan's algorithm
	var dependencies = get_variable_dependencies()
	var visited = {}
	var visiting = {}
	
	# DFS to detect cycles
	for var_name in dependencies:
		if not var_name in visited:
			if _has_cycle(var_name, dependencies, visited, visiting):
				return true
	
	return false

# Helper for cycle detection
func _has_cycle(var_name: String, dependencies: Dictionary, visited: Dictionary, visiting: Dictionary) -> bool:
	visiting[var_name] = true
	
	if var_name in dependencies:
		for eq_idx in dependencies[var_name]:
			var eq = equations[eq_idx]
			for dep_var in eq.get_involved_variables():
				if dep_var != var_name:  # Avoid self-dependency
					if dep_var in visiting:
						return true  # Found a cycle
					if not dep_var in visited:
						if _has_cycle(dep_var, dependencies, visited, visiting):
							return true
	
	visiting.erase(var_name)
	visited[var_name] = true
	return false

# String representation for debugging
func _to_string() -> String:
	var s = "EquationSystem with %d equations and %d variables\n" % [equations.size(), variables.size()]
	
	s += "Variables:\n"
	for name in variables:
		var var_data = variables[name]
		s += "  %s = %f (state=%s, param=%s)\n" % [
			name,
			var_data.value,
			"true" if var_data.is_state else "false",
			"true" if var_data.is_parameter else "false"
		]
	
	s += "Equations:\n"
	for i in range(equations.size()):
		s += "  [%d] %s\n" % [i, str(equations[i])]
	
	return s 