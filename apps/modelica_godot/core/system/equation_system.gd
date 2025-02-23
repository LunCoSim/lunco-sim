class_name ModelicaEquationSystem
extends ModelicaBase

const ImprovedASTNode = preload("./ast_node.gd")

var components: Array[ModelicaComponent] = []
var connection_sets: Array = []          # From connect equations
var initial_system: Dictionary = {}      # For initialization
var runtime_system: Dictionary = {}      # For simulation
var time: float = 0.0
var dt: float = 0.01

# Solver settings
const MAX_ITERATIONS = 50
const TOLERANCE = 1e-8

func _init() -> void:
	initial_system = {
		"equations": [],
		"variables": {},
		"parameters": {}
	}
	runtime_system = {
		"equations": [],
		"variables": {},
		"parameters": {},
		"state_variables": {},
		"derivatives": {}
	}

func add_component(component: ModelicaComponent) -> void:
	components.append(component)
	
	# Register all variables and parameters
	for var_name in component.variables:
		var var_obj = component.get_variable(var_name)
		if var_obj.is_state_variable():
			runtime_system.state_variables[var_name] = var_obj
		else:
			runtime_system.variables[var_name] = var_obj
	
	for param_name in component.parameters:
		var param_obj = component.get_parameter(param_name)
		runtime_system.parameters[param_name] = param_obj
	
	# Add equations
	runtime_system.equations.extend(component.get_equations())
	initial_system.equations.extend(component.get_initial_equations())

func connect(conn1: ModelicaConnector, conn2: ModelicaConnector) -> void:
	conn1.connect_to(conn2)
	connection_sets.append([conn1, conn2])
	
	# Generate connection equations
	for var_name in conn1.variables:
		if conn2.has_variable(var_name):
			var var1 = conn1.get_variable(var_name)
			var var2 = conn2.get_variable(var_name)
			
			if var1.is_flow_variable():
				# Sum of flow variables = 0
				runtime_system.equations.append("%s + %s = 0" % [var1.get_declaration(var1.declarations.keys()[0]).name, 
															   var2.get_declaration(var2.declarations.keys()[0]).name])
			else:
				# Equality of potential variables
				runtime_system.equations.append("%s = %s" % [var1.get_declaration(var1.declarations.keys()[0]).name,
														   var2.get_declaration(var2.declarations.keys()[0]).name])

func solve_initialization() -> bool:
	# First solve initial equations
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		var new_values = {}
		
		# Evaluate all initial equations
		for eq in initial_system.equations:
			var ast_dict = parse_expression(tokenize(eq))
			if ast_dict.node == null:
				push_error("Failed to parse equation: " + eq)
				return false
			
			var rhs_value = evaluate_ast(ast_dict.node)
			var lhs_var = ast_dict.node.left.value if ast_dict.node.left else ""
			if lhs_var != "":
				new_values[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - get_variable_value(lhs_var)))
		
		# Update state
		for var_name in new_values:
			set_variable_value(var_name, new_values[var_name])
		
		if max_residual < TOLERANCE:
			return true
	
	return false

func solve_step() -> bool:
	# Store current state
	var current_state = {}
	for var_name in runtime_system.variables:
		current_state[var_name] = runtime_system.variables[var_name].value
	for var_name in runtime_system.state_variables:
		current_state[var_name] = runtime_system.state_variables[var_name].value
	
	# First solve algebraic equations
	if not _solve_algebraic_equations(current_state):
		return false
	
	# Evaluate derivatives at current state
	var current_derivatives = {}
	for var_name in runtime_system.state_variables:
		var der_name = "der(" + var_name + ")"
		var der_eq = _find_derivative_equation(der_name)
		if der_eq != "":
			var ast_dict = parse_expression(tokenize(der_eq))
			if ast_dict.node != null:
				current_derivatives[var_name] = evaluate_ast(ast_dict.node)
	
	# Perform RK4 integration for state variables
	var k1_state = current_state.duplicate()
	var k2_state = current_state.duplicate()
	var k3_state = current_state.duplicate()
	var k4_state = current_state.duplicate()
	
	# k1 = f(t, y)
	var k1 = current_derivatives
	
	# k2 = f(t + dt/2, y + dt*k1/2)
	for var_name in k1:
		k2_state[var_name] = current_state[var_name] + 0.5 * dt * k1[var_name]
	_solve_algebraic_equations(k2_state)
	var k2 = {}
	for var_name in runtime_system.state_variables:
		var der_name = "der(" + var_name + ")"
		var der_eq = _find_derivative_equation(der_name)
		if der_eq != "":
			var ast_dict = parse_expression(tokenize(der_eq))
			if ast_dict.node != null:
				k2[var_name] = evaluate_ast(ast_dict.node)
	
	# k3 = f(t + dt/2, y + dt*k2/2)
	for var_name in k2:
		k3_state[var_name] = current_state[var_name] + 0.5 * dt * k2[var_name]
	_solve_algebraic_equations(k3_state)
	var k3 = {}
	for var_name in runtime_system.state_variables:
		var der_name = "der(" + var_name + ")"
		var der_eq = _find_derivative_equation(der_name)
		if der_eq != "":
			var ast_dict = parse_expression(tokenize(der_eq))
			if ast_dict.node != null:
				k3[var_name] = evaluate_ast(ast_dict.node)
	
	# k4 = f(t + dt, y + dt*k3)
	for var_name in k3:
		k4_state[var_name] = current_state[var_name] + dt * k3[var_name]
	_solve_algebraic_equations(k4_state)
	var k4 = {}
	for var_name in runtime_system.state_variables:
		var der_name = "der(" + var_name + ")"
		var der_eq = _find_derivative_equation(der_name)
		if der_eq != "":
			var ast_dict = parse_expression(tokenize(der_eq))
			if ast_dict.node != null:
				k4[var_name] = evaluate_ast(ast_dict.node)
	
	# Update state variables using RK4 formula
	# y(t + dt) = y(t) + (dt/6)*(k1 + 2*k2 + 2*k3 + k4)
	for var_name in k1:
		var new_value = current_state[var_name] + (dt / 6.0) * (
			k1[var_name] + 2.0 * k2[var_name] + 2.0 * k3[var_name] + k4[var_name]
		)
		set_variable_value(var_name, new_value)
	
	# Final algebraic solve to ensure consistency
	if not _solve_algebraic_equations(runtime_system.variables):
		return false
	
	time += dt
	return true

func _solve_algebraic_equations(state: Dictionary) -> bool:
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		var new_values = {}
		
		# Evaluate all algebraic equations
		for eq in runtime_system.equations:
			if not _is_derivative_equation(eq):
				var ast_dict = parse_expression(tokenize(eq))
				if ast_dict.node == null:
					push_error("Failed to parse equation: " + eq)
					return false
				
				var rhs_value = evaluate_ast(ast_dict.node)
				var lhs_var = ast_dict.node.left.value if ast_dict.node.left else ""
				if lhs_var != "":
					new_values[lhs_var] = rhs_value
					max_residual = max(max_residual, abs(rhs_value - get_variable_value(lhs_var)))
		
		# Update state
		for var_name in new_values:
			set_variable_value(var_name, new_values[var_name])
		
		if max_residual < TOLERANCE:
			return true
	
	return false

func _is_derivative_equation(eq: String) -> bool:
	return eq.begins_with("der(")

func _find_derivative_equation(der_name: String) -> String:
	for eq in runtime_system.equations:
		if eq.begins_with(der_name):
			return eq
	return ""

func get_variable_value(name: String) -> float:
	if runtime_system.variables.has(name):
		return runtime_system.variables[name].value
	elif runtime_system.state_variables.has(name):
		return runtime_system.state_variables[name].value
	elif runtime_system.parameters.has(name):
		return runtime_system.parameters[name].value
	return 0.0

func set_variable_value(name: String, value: float) -> void:
	if runtime_system.variables.has(name):
		runtime_system.variables[name].set_value(value)
	elif runtime_system.state_variables.has(name):
		runtime_system.state_variables[name].set_value(value)

func _to_string() -> String:
	var result = "ModelicaEquationSystem:\n"
	result += "  Time: %f\n" % time
	result += "  Components: %d\n" % components.size()
	result += "  Connection Sets: %d\n" % connection_sets.size()
	result += "  Variables: %d\n" % runtime_system.variables.size()
	result += "  State Variables: %d\n" % runtime_system.state_variables.size()
	result += "  Parameters: %d\n" % runtime_system.parameters.size()
	result += "  Equations: %d\n" % runtime_system.equations.size()
	return result

