class_name RK4Solver
extends RefCounted

# The equation system this solver operates on
var equation_system = null

# State variables (variables that have derivatives)
var state_variables = []

# Derivative equations for each state variable
var derivative_equations = {}

# A causal solver for algebraic variables
var algebraic_solver = null

# Execution flags
var initialized: bool = false

# Initialize with an equation system
func initialize(eq_system) -> bool:
	equation_system = eq_system
	return _initialize_impl()

# Override initialization
func _initialize_impl() -> bool:
	if equation_system == null:
		push_error("Cannot initialize: equation system is null")
		return false
	
	# Identify state variables and their derivative equations
	var success = identify_state_variables()
	if not success:
		return false
	
	# Set up algebraic solver for non-state variables
	success = setup_algebraic_solver()
	
	initialized = success
	return success

# Identify state variables and their derivative equations
func identify_state_variables() -> bool:
	state_variables = equation_system.get_state_variables()
	
	if state_variables.size() == 0:
		push_warning("No state variables found in the system")
	
	# Find derivative equations for each state variable
	for eq in equation_system.equations:
		if eq.type == ModelicaEquation.EquationType.DIFFERENTIAL:
			# This should be an equation like der(x) = f(...)
			if eq.left_expression.type == ModelicaExpression.ExpressionType.DERIVATIVE:
				if eq.left_expression.arguments.size() > 0:
					var state_var = eq.left_expression.arguments[0].value
					if state_var in state_variables:
						derivative_equations[state_var] = eq
					else:
						push_warning("Derivative equation for non-state variable: " + state_var)
	
	# Check that we have equations for all state variables
	for var_name in state_variables:
		if not var_name in derivative_equations:
			push_error("Missing derivative equation for state variable: " + var_name)
			return false
	
	return true

# Set up algebraic solver for non-state variables
func setup_algebraic_solver() -> bool:
	# Create a new equation system for algebraic variables only
	var algebraic_system = EquationSystem.new()
	
	# Copy all variables except state variables
	for var_name in equation_system.variables:
		if not var_name in state_variables:
			var var_data = equation_system.variables[var_name]
			algebraic_system.add_variable(var_name, var_data)
	
	# Copy all equations except derivative equations
	for eq in equation_system.equations:
		if eq.type != ModelicaEquation.EquationType.DIFFERENTIAL:
			algebraic_system.add_equation(eq)
	
	# Create and initialize the causal solver
	algebraic_solver = CausalSolver.new()
	return algebraic_solver.initialize(algebraic_system)

# Implement a step of the solver
func step(dt: float) -> bool:
	if not initialized:
		push_error("Cannot step: solver not initialized")
		return false
	
	if equation_system == null:
		push_error("Cannot step: equation system is null")
		return false
	
	return _step_impl(dt)

# Implementation of the step
func _step_impl(dt: float) -> bool:
	# Perform RK4 integration for all state variables
	var current_state = {}
	
	# Copy current variable values
	for var_name in equation_system.variables:
		current_state[var_name] = equation_system.variables[var_name].value
	
	# RK4 integration
	var k1 = calculate_derivatives(current_state)
	
	var k2_state = current_state.duplicate()
	for state_var in state_variables:
		k2_state[state_var] += k1[state_var] * dt / 2.0
	var k2 = calculate_derivatives(k2_state)
	
	var k3_state = current_state.duplicate()
	for state_var in state_variables:
		k3_state[state_var] += k2[state_var] * dt / 2.0
	var k3 = calculate_derivatives(k3_state)
	
	var k4_state = current_state.duplicate()
	for state_var in state_variables:
		k4_state[state_var] += k3[state_var] * dt
	var k4 = calculate_derivatives(k4_state)
	
	# Apply RK4 formula to update state variables
	for state_var in state_variables:
		var new_value = current_state[state_var] + dt/6.0 * (
			k1[state_var] + 2*k2[state_var] + 2*k3[state_var] + k4[state_var]
		)
		equation_system.set_variable_value(state_var, new_value)
	
	# Update algebraic variables with the new state
	if algebraic_solver != null:
		# Copy updated state variables to the algebraic system
		for state_var in state_variables:
			algebraic_solver.equation_system.set_variable_value(
				state_var, 
				equation_system.get_variable_value(state_var)
			)
		
		# Step the algebraic solver to update non-state variables
		algebraic_solver.step(0.0)  # dt=0 because we're just updating, not advancing time
		
		# Copy back the updated algebraic variables
		for var_name in algebraic_solver.equation_system.variables:
			equation_system.set_variable_value(
				var_name, 
				algebraic_solver.equation_system.get_variable_value(var_name)
			)
	
	# Update time
	equation_system.time += dt
	
	return true

# Calculate derivatives for all state variables given a state
func calculate_derivatives(state_values: Dictionary) -> Dictionary:
	var derivatives = {}
	
	# For each state variable, evaluate its derivative equation
	for state_var in derivative_equations:
		var eq = derivative_equations[state_var]
		
		# The equation is der(x) = f(...), so evaluate the right-hand side
		var der_value = eq.right_expression.evaluate(state_values)
		derivatives[state_var] = der_value
	
	return derivatives

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

# Create string representation for debugging
func _to_string() -> String:
	var s = "RK4Solver (initialized=%s)\n" % (str(initialized))
	
	s += "State Variables:\n"
	for var_name in state_variables:
		s += "  %s\n" % var_name
	
	s += "Derivative Equations:\n"
	for var_name in derivative_equations:
		s += "  der(%s) = %s\n" % [var_name, str(derivative_equations[var_name].right_expression)]
	
	return s 