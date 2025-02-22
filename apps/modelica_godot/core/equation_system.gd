class_name EquationSystem
extends Node

const ImprovedASTNode = preload("res://apps/modelica_godot/core/ast_node.gd")

# System state
var time: float = 0.0
var dt: float = 0.01  # Time step size
var equations: Array[Dictionary] = []
var variables: Dictionary = {}
var derivatives: Dictionary = {}
var components: Array[ModelicaComponent] = []
var initial_conditions: Array[Dictionary] = []

# Solver settings
const MAX_ITERATIONS = 50
const TOLERANCE = 1e-8  # Increased precision

class EquationToken:
	var type: String
	var value: String
	
	func _init(p_type: String, p_value: String):
		type = p_type
		value = p_value
	
	func _to_string() -> String:
		return "Token(%s, '%s')" % [type, value]

const TOKEN_TYPES = {
	"NUMBER": "\\d+(\\.\\d+)?",
	"OPERATOR": "[+\\-*/^]",
	"EQUALS": "=",
	"LPAREN": "\\(",
	"RPAREN": "\\)",
	"IDENTIFIER": "[a-zA-Z_][a-zA-Z0-9_]*",
	"DOT": "\\.",
	"COMMA": ",",
	"WHITESPACE": "\\s+"
}

func _init() -> void:
	equations = []
	variables = {}
	derivatives = {}
	components = []
	initial_conditions = []
	time = 0.0

func _ready() -> void:
	print("EquationSystem: Ready")

func clear() -> void:
	equations = []
	variables = {}
	derivatives = {}
	components = []
	initial_conditions = []
	time = 0.0

func add_equation(equation: String, component: ModelicaComponent) -> void:
	# Parse equation and identify if it's differential
	var parts = equation.split("=")
	if parts.size() != 2:
		push_error("Invalid equation format: " + equation)
		return
	
	var left = parts[0].strip_edges()
	var right = parts[1].strip_edges()
	
	# Check if it's a differential equation (contains der())
	var is_differential = left.begins_with("der(") or right.begins_with("der(")
	
	equations.append({
		"left": left,
		"right": right,
		"component": component,
		"is_differential": is_differential
	})

func add_initial_condition(variable: String, value: float, component: ModelicaComponent) -> void:
	initial_conditions.append({
		"variable": variable,
		"value": value,
		"component": component
	})

func add_component(component: ModelicaComponent) -> void:
	if not components.has(component):
		components.append(component)

func initialize() -> void:
	# First apply initial conditions
	for ic in initial_conditions:
		var var_name = ic.variable
		var value = ic.value
		var component = ic.component
		
		# Update both system and component state
		variables[var_name] = value
		
		# Update component if this is a component variable
		var parts = var_name.split(".")
		if parts.size() == 2:
			var comp_name = parts[0]
			var var_name_only = parts[1]
			if component.component_name == comp_name:
				component.set_variable(var_name_only, value)
	
	# Solve initial system of equations
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		var new_values = {}
		
		# First pass: evaluate all equations
		for eq in equations:
			if not eq.is_differential:
				var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
				var lhs_var = eq.left.strip_edges()
				new_values[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - variables.get(lhs_var, 0.0)))
		
		# Update state with new values
		for var_name in new_values:
			variables[var_name] = new_values[var_name]
			
			# Update component state if needed
			var parts = var_name.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name_only = parts[1]
				for comp in components:
					if comp.component_name == comp_name:
						comp.set_variable(var_name_only, new_values[var_name])
						break
		
		if max_residual < TOLERANCE:
			break
	
	# Initialize derivatives
	derivatives.clear()
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
			derivatives[der_var] = rhs_value
			print("Initial derivative for %s = %f" % [der_var, rhs_value])

func solve_step() -> void:
	# Store current state
	var current_state = variables.duplicate()
	
	# First solve algebraic equations to get consistent state
	_solve_algebraic_equations(current_state)
	
	# Evaluate derivatives at current state
	var current_derivatives = {}
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, current_state)
			current_derivatives[der_var] = rhs_value
	
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
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, k2_state)
			k2[der_var] = rhs_value
	
	# k3 = f(t + dt/2, y + dt*k2/2)
	for var_name in k2:
		k3_state[var_name] = current_state[var_name] + 0.5 * dt * k2[var_name]
	_solve_algebraic_equations(k3_state)
	var k3 = {}
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, k3_state)
			k3[der_var] = rhs_value
	
	# k4 = f(t + dt, y + dt*k3)
	for var_name in k3:
		k4_state[var_name] = current_state[var_name] + dt * k3[var_name]
	_solve_algebraic_equations(k4_state)
	var k4 = {}
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, k4_state)
			k4[der_var] = rhs_value
	
	# Update state variables using RK4 formula
	# y(t + dt) = y(t) + (dt/6)*(k1 + 2*k2 + 2*k3 + k4)
	for var_name in k1:
		var new_value = current_state[var_name] + (dt / 6.0) * (
			k1[var_name] + 2.0 * k2[var_name] + 2.0 * k3[var_name] + k4[var_name]
		)
		variables[var_name] = new_value
		
		# Update component state
		var parts = var_name.split(".")
		if parts.size() >= 2:
			var comp_name = parts[0]
			var var_name_only = parts[1]
			for comp in components:
				if comp.component_name == comp_name:
					comp.set_variable(var_name_only, new_value)
					break
	
	# Final algebraic solve to ensure consistency
	_solve_algebraic_equations(variables)
	
	# Update component states for algebraic variables
	for var_name in variables:
		var parts = var_name.split(".")
		if parts.size() >= 2:
			var comp_name = parts[0]
			var var_name_only = parts[1]
			for comp in components:
				if comp.component_name == comp_name:
					comp.set_variable(var_name_only, variables[var_name])
					break
	
	# Store derivatives for next step
	derivatives.clear()
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
			derivatives[der_var] = rhs_value
	
	time += dt

func _solve_algebraic_equations(state: Dictionary) -> void:
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		var new_values = {}
		
		# Evaluate all algebraic equations
		for eq in equations:
			if not eq.is_differential:
				var rhs_value = _evaluate_expression(eq.right, eq.component, state)
				var lhs_var = eq.left.strip_edges()
				new_values[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - state.get(lhs_var, 0.0)))
		
		# Update state with new values
		for var_name in new_values:
			state[var_name] = new_values[var_name]
		
		if max_residual < TOLERANCE:
			break

func _evaluate_expression(expression: String, component: ModelicaComponent, state: Dictionary) -> float:
	var tokens = tokenize(expression)
	var ast_dict = parse_expression(tokens)
	
	if ast_dict.node == null:
		push_error("Failed to parse expression: " + expression)
		return 0.0
	
	return evaluate_ast(ast_dict.node, component, state)

func evaluate_ast(node: ImprovedASTNode, component: ModelicaComponent, state: Dictionary) -> float:
	if node == null:
		return 0.0
		
	match node.type:
		"NUMBER":
			return float(node.value)
			
		"VARIABLE":
			# Get the full variable name from dependencies
			var var_name = node.dependencies[0] if node.dependencies.size() > 0 else node.value
			
			if "." in var_name:
				# Handle component references (e.g., mass1.position or mass1.port.position)
				var parts = var_name.split(".")
				var comp_name = parts[0]
				
				# First try to get from state dictionary
				if state.has(var_name):
					return state[var_name]
				
				# Then try to find the referenced component
				for comp in components:
					if comp.component_name == comp_name:
						if parts.size() == 2:
							# Simple component variable (e.g., mass1.position)
							var var_name_only = parts[1]
							# Try parameter first, then variable
							var param_value = comp.get_parameter(var_name_only)
							if param_value != null:
								return param_value
							var var_value = comp.get_variable(var_name_only)
							if var_value != null:
								return var_value
						elif parts.size() == 3:
							# Port variable (e.g., mass1.port.position)
							var port_name = parts[1]
							var var_name_only = parts[2]
							var full_var_name = port_name + "." + var_name_only
							var var_value = comp.get_variable(full_var_name)
							if var_value != null:
								return var_value
						push_error("Variable not found in component: " + var_name)
						return 0.0
				
				push_error("Component not found: " + comp_name)
				return 0.0
			else:
				# First try to get from state dictionary
				if state.has(var_name):
					return state[var_name]
				
				# Then try parameter first, then variable from the component
				var param_value = component.get_parameter(var_name)
				if param_value != null:
					return param_value
				var var_value = component.get_variable(var_name)
				if var_value != null:
					return var_value
				push_error("Variable not found: " + var_name)
				return 0.0
				
		"BINARY_OP":
			var left_val = evaluate_ast(node.left, component, state)
			var right_val = evaluate_ast(node.right, component, state)
			
			match node.value:
				"+": return left_val + right_val
				"-": return left_val - right_val
				"*": return left_val * right_val
				"/": return float(left_val) / float(right_val)  # Ensure float division
				"^": return pow(left_val, right_val)
				_:
					push_error("Unknown binary operator: " + node.value)
					return 0.0
					
		"UNARY_OP":
			var operand_val = evaluate_ast(node.operand, component, state)
			match node.value:
				"+": return operand_val
				"-": return -operand_val
				_:
					push_error("Unknown unary operator: " + node.value)
					return 0.0
					
		"FUNCTION_CALL":
			var args = []
			for arg in node.arguments:
				args.append(evaluate_ast(arg, component, state))
				
			match node.value:
				"sin": return sin(args[0])
				"cos": return cos(args[0])
				"tan": return tan(args[0])
				"sqrt": return sqrt(args[0])
				"der":
					if args.size() > 0:
						var var_name = node.state_variable
						if derivatives.has(var_name):
							return derivatives[var_name]
						push_error("Derivative not found for: " + var_name)
					return 0.0
				_:
					push_error("Unknown function: " + node.value)
					return 0.0
					
		_:
			push_error("Unknown node type: " + node.type)
			return 0.0

func _extract_der_variable(expr: String) -> String:
	# Extract variable name from der() expression
	var start_idx = expr.find("der(") + 4
	var end_idx = expr.find(")", start_idx)
	if start_idx >= 4 and end_idx > start_idx:
		return expr.substr(start_idx, end_idx - start_idx).strip_edges()
	return ""

func tokenize(expression: String) -> Array:
	var tokens = []
	var pos = 0
	
	while pos < expression.length():
		var matched = false
		
		# Skip whitespace
		if expression[pos] == " ":
			pos += 1
			continue
		
		# Try to match each token type
		for type in TOKEN_TYPES:
			var regex = RegEx.new()
			regex.compile(TOKEN_TYPES[type])
			var result = regex.search(expression, pos)
			
			if result and result.get_start() == pos:
				tokens.append(EquationToken.new(type, result.get_string()))
				pos = result.get_end()
				matched = true
				break
		
		if not matched:
			push_error("Invalid token at position " + str(pos))
			return []
	
	return tokens

func parse_expression(tokens: Array) -> Dictionary:
	var pos = 0
	var result = _parse_term(tokens, pos)
	return {
		"node": result.node,
		"pos": result.pos
	}

func _parse_term(tokens: Array, pos: int) -> Dictionary:
	var result = _parse_factor(tokens, pos)
	var left = result.node
	pos = result.pos
	
	while pos < tokens.size() and tokens[pos].type == "OPERATOR" and (tokens[pos].value == "+" or tokens[pos].value == "-"):
		var op = tokens[pos].value
		pos += 1
		result = _parse_factor(tokens, pos)
		var right = result.node
		pos = result.pos
		
		var node = ImprovedASTNode.new("BINARY_OP", op)
		node.left = left
		node.right = right
		left = node
	
	return {"node": left, "pos": pos}

func _parse_factor(tokens: Array, pos: int) -> Dictionary:
	var result = _parse_power(tokens, pos)
	var left = result.node
	pos = result.pos
	
	while pos < tokens.size() and tokens[pos].type == "OPERATOR" and (tokens[pos].value == "*" or tokens[pos].value == "/"):
		var op = tokens[pos].value
		pos += 1
		result = _parse_power(tokens, pos)
		var right = result.node
		pos = result.pos
		
		var node = ImprovedASTNode.new("BINARY_OP", op)
		node.left = left
		node.right = right
		left = node
	
	return {"node": left, "pos": pos}

func _parse_power(tokens: Array, pos: int) -> Dictionary:
	var result = _parse_primary(tokens, pos)
	var left = result.node
	pos = result.pos
	
	while pos < tokens.size() and tokens[pos].type == "OPERATOR" and tokens[pos].value == "^":
		var op = tokens[pos].value
		pos += 1
		result = _parse_power(tokens, pos)  # Right associative
		var right = result.node
		pos = result.pos
		
		var node = ImprovedASTNode.new("BINARY_OP", op)
		node.left = left
		node.right = right
		left = node
	
	return {"node": left, "pos": pos}

func _parse_primary(tokens: Array, pos: int) -> Dictionary:
	if pos >= tokens.size():
		return {"node": null, "pos": pos}
	
	var token = tokens[pos]
	
	match token.type:
		"NUMBER":
			var node = ImprovedASTNode.new("NUMBER", token.value)
			return {"node": node, "pos": pos + 1}
			
		"IDENTIFIER":
			if pos + 1 < tokens.size() and tokens[pos + 1].type == "LPAREN":
				# Function call
				var node = ImprovedASTNode.new("FUNCTION_CALL", token.value)
				pos += 2  # Skip identifier and left paren
				
				while pos < tokens.size() and tokens[pos].type != "RPAREN":
					var arg_result = _parse_term(tokens, pos)
					if arg_result.node != null:
						node.arguments.append(arg_result.node)
					pos = arg_result.pos
					
					if pos < tokens.size() and tokens[pos].type == "COMMA":
						pos += 1
				
				if pos < tokens.size() and tokens[pos].type == "RPAREN":
					pos += 1
				
				if token.value == "der":
					node.is_differential = true
					if node.arguments.size() > 0:
						var arg = node.arguments[0]
						if arg.type == "VARIABLE":
							node.state_variable = arg.value
						elif arg.type == "BINARY_OP" and arg.left and arg.left.type == "VARIABLE":
							node.state_variable = arg.left.value
				
				return {"node": node, "pos": pos}
			else:
				# Variable
				var node = ImprovedASTNode.new("VARIABLE", token.value)
				pos += 1
				
				# Handle dot notation (e.g., mass.position)
				while pos < tokens.size() and tokens[pos].type == "DOT":
					pos += 1
					if pos < tokens.size() and tokens[pos].type == "IDENTIFIER":
						node.value += "." + tokens[pos].value
						pos += 1
					else:
						push_error("Expected identifier after dot")
						return {"node": null, "pos": pos}
				
				return {"node": node, "pos": pos}
				
		"LPAREN":
			pos += 1
			var result = _parse_term(tokens, pos)
			pos = result.pos
			
			if pos < tokens.size() and tokens[pos].type == "RPAREN":
				pos += 1
				return {"node": result.node, "pos": pos}
			else:
				push_error("Expected closing parenthesis")
				return {"node": null, "pos": pos}
				
		"OPERATOR":
			if token.value == "+" or token.value == "-":
				# Unary operator
				pos += 1
				var result = _parse_primary(tokens, pos)
				var node = ImprovedASTNode.new("UNARY_OP", token.value)
				node.operand = result.node
				return {"node": node, "pos": result.pos}
		
		_:
			push_error("Unexpected token type: " + token.type)
			return {"node": null, "pos": pos}
	
	return {"node": null, "pos": pos}

