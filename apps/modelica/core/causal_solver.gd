class_name CausalSolver
extends RefCounted

var equation_system = null
var evaluation_sequence = []
var initialized: bool = false

# Initialize the solver with an equation system
func initialize(eq_system) -> bool:
	equation_system = eq_system
	return _initialize_impl()

# Implementation of initialization
func _initialize_impl() -> bool:
	if equation_system == null:
		push_error("Cannot initialize: equation system is null")
		return false
	
	# Generate the evaluation sequence
	var success = generate_evaluation_sequence()
	
	initialized = success
	return success

# Generate the evaluation sequence by determining the order in which
# variables should be evaluated based on equation dependencies
func generate_evaluation_sequence() -> bool:
	evaluation_sequence = []
	
	# Start with known variables (parameters)
	var known_variables = {}
	var parameters = equation_system.get_parameters()
	
	for param in parameters:
		known_variables[param] = true
	
	# Keep track of equations we've already processed
	var used_equations = {}
	var all_equations = equation_system.equations
	
	# Continue until we can't solve for any more variables
	var progress = true
	while progress:
		progress = false
		
		# Search for equations that can be solved for a single unknown
		for eq_idx in range(all_equations.size()):
			if eq_idx in used_equations:
				continue
			
			var eq = all_equations[eq_idx]
			var unknown_vars = []
			
			# Check which variables in this equation are unknown
			for var_name in eq.get_involved_variables():
				if not var_name in known_variables:
					unknown_vars.append(var_name)
			
			# If we have exactly one unknown variable, add to evaluation sequence
			if unknown_vars.size() == 1:
				var var_to_solve = unknown_vars[0]
				
				evaluation_sequence.append({
					"equation": eq,
					"variable": var_to_solve
				})
				
				known_variables[var_to_solve] = true
				used_equations[eq_idx] = true
				progress = true
		
		# If we didn't make progress but haven't processed all equations,
		# there might be an algebraic loop
		if not progress and used_equations.size() < all_equations.size():
			push_error("Possible algebraic loop detected. Cannot solve system causally.")
			return false
	
	# Check if we found a solution for all variables
	var all_vars = equation_system.variables.keys()
	for var_name in all_vars:
		if not var_name in known_variables and not equation_system.variables[var_name].is_parameter:
			push_error("Could not solve for variable: " + var_name)
			return false
	
	return true

# Take a step in time
func step(dt: float) -> bool:
	if not initialized:
		push_error("Cannot step: solver not initialized")
		return false
	
	if equation_system == null:
		push_error("Cannot step: equation system is null")
		return false
	
	# Execute the evaluation sequence
	for item in evaluation_sequence:
		var eq = item.equation
		var var_name = item.variable
		
		# Get current variable values
		var current_values = {}
		for vname in equation_system.variables:
			current_values[vname] = equation_system.get_variable_value(vname)
		
		# Solve the equation for the variable
		var new_value = eq.solve_for(var_name, current_values)
		
		# Update the variable
		equation_system.set_variable_value(var_name, new_value)
	
	# Update time
	equation_system.time += dt
	
	return true

# Get the current state
func get_state() -> Dictionary:
	if equation_system != null:
		return equation_system.get_state()
	return {}

# Set the state
func set_state(state: Dictionary) -> void:
	if equation_system != null:
		equation_system.set_state(state)

# Reset the solver
func reset() -> void:
	if equation_system != null:
		equation_system.reset()
	
	# Re-initialize
	_initialize_impl()

# String representation for debugging
func _to_string() -> String:
	var s = "CausalSolver (initialized=%s)\n" % (str(initialized))
	
	s += "Evaluation Sequence:\n"
	for i in range(evaluation_sequence.size()):
		var item = evaluation_sequence[i]
		s += "  [%d] %s -> %s\n" % [i, str(item.equation), item.variable]
	
	return s 