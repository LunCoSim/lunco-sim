class_name EquationSystem
extends Node

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

# RK4 integration helper function
func _rk4_step(current_state: Dictionary, current_derivatives: Dictionary) -> Dictionary:
	var k1 = current_derivatives.duplicate()
	var k2_state = current_state.duplicate()
	var k3_state = current_state.duplicate()
	var k4_state = current_state.duplicate()
	
	# Calculate k1 (already done)
	
	# Calculate k2 (midpoint)
	for var_name in k1:
		k2_state[var_name] = current_state[var_name] + 0.5 * dt * k1[var_name]
	var k2 = _evaluate_derivatives(k2_state)
	
	# Calculate k3 (midpoint)
	for var_name in k2:
		k3_state[var_name] = current_state[var_name] + 0.5 * dt * k2[var_name]
	var k3 = _evaluate_derivatives(k3_state)
	
	# Calculate k4 (endpoint)
	for var_name in k3:
		k4_state[var_name] = current_state[var_name] + dt * k3[var_name]
	var k4 = _evaluate_derivatives(k4_state)
	
	# Combine all steps
	var next_state = current_state.duplicate()
	for var_name in current_derivatives:
		next_state[var_name] = current_state[var_name] + (dt / 6.0) * (
			k1[var_name] + 2 * k2[var_name] + 2 * k3[var_name] + k4[var_name]
		)
	
	return next_state

func _evaluate_derivatives(state: Dictionary) -> Dictionary:
	var new_derivatives = {}
	for eq in equations:
		if eq.is_differential:
			var rhs_value = _evaluate_expression(eq.right, eq.component, state)
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			new_derivatives[der_var] = rhs_value
	return new_derivatives

func solve_step() -> void:
	# Solve one time step using RK4 method
	# 1. Store current state
	var current_state = variables.duplicate()
	var current_derivatives = derivatives.duplicate()
	
	# 2. Solve the nonlinear system at the current time
	var converged = false
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		var new_derivatives = {}
		var new_values = {}
		
		# First pass: evaluate all equations
		for eq in equations:
			var rhs_value = _evaluate_expression(eq.right, eq.component, current_state)
			
			if eq.is_differential:
				# Handle differential equations
				var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
				new_derivatives[der_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - current_derivatives.get(der_var, 0.0)))
			else:
				# Handle algebraic equations
				var lhs_var = eq.left
				new_values[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - current_state.get(lhs_var, 0.0)))
		
		# Second pass: update all variables
		for var_name in new_values:
			current_state[var_name] = new_values[var_name]
			
			# Update component state
			var parts = var_name.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name_only = parts[1]
				for comp in components:
					if comp.component_name == comp_name:
						comp.set_variable(var_name_only, new_values[var_name])
						break
		
		# Update derivatives
		current_derivatives = new_derivatives
		
		if max_residual < TOLERANCE:
			converged = true
			break
	
	if not converged:
		push_warning("Solver did not converge in " + str(MAX_ITERATIONS) + " iterations")
	
	# 3. Update state using RK4 integration
	var next_state = _rk4_step(current_state, current_derivatives)
	
	# Update component states
	for var_name in next_state:
		var parts = var_name.split(".")
		if parts.size() == 2:
			var comp_name = parts[0]
			var var_name_only = parts[1]
			for comp in components:
				if comp.component_name == comp_name:
					comp.set_variable(var_name_only, next_state[var_name])
					break
	
	# 4. Update system state
	variables = next_state
	derivatives = current_derivatives
	
	# 5. Solve algebraic equations one more time to ensure consistency
	var final_values = {}
	for eq in equations:
		if not eq.is_differential:
			var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
			var lhs_var = eq.left
			final_values[lhs_var] = rhs_value
			
			# Update both system and component state
			variables[lhs_var] = rhs_value
			var parts = lhs_var.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name_only = parts[1]
				for comp in components:
					if comp.component_name == comp_name:
						comp.set_variable(var_name_only, rhs_value)
						break
	
	time += dt

func initialize() -> void:
	# Apply initial conditions
	for ic in initial_conditions:
		var parts = ic.variable.split(".")
		if parts.size() == 2:
			var comp_name = parts[0]
			var var_name = parts[1]
			for comp in components:
				if comp.component_name == comp_name:
					comp.set_variable(var_name, ic.value)
					variables[ic.variable] = ic.value
					break
	
	# Solve initial system
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		
		for eq in equations:
			if not eq.is_differential:
				var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
				var lhs_var = eq.left
				
				# Update both system and component state
				variables[lhs_var] = rhs_value
				var parts = lhs_var.split(".")
				if parts.size() == 2:
					var comp_name = parts[0]
					var var_name = parts[1]
					for comp in components:
						if comp.component_name == comp_name:
							comp.set_variable(var_name, rhs_value)
							break
				
				max_residual = max(max_residual, abs(rhs_value - variables.get(lhs_var, 0.0)))
		
		if max_residual < TOLERANCE:
			break
	
	# Initialize derivatives
	for eq in equations:
		if eq.is_differential:
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
			derivatives[der_var] = rhs_value

func tokenize(expr: String) -> Array[EquationToken]:
	var tokens: Array[EquationToken] = []
	var pos = 0
	
	while pos < expr.length():
		var matched = false
		# Skip whitespace
		if expr[pos] == " ":
			pos += 1
			continue
			
		for type in TOKEN_TYPES:
			var regex = RegEx.new()
			regex.compile("^" + TOKEN_TYPES[type])
			var result = regex.search(expr.substr(pos))
			
			if result:
				var value = result.get_string()
				if type != "WHITESPACE":  # Skip whitespace tokens
					tokens.append(EquationToken.new(type, value))
				pos += value.length()
				matched = true
				break
		
		if not matched:
			push_error("Invalid character in expression at position %d: %s" % [pos, expr[pos]])
			return []
	
	return tokens

func parse_expression(tokens: Array[EquationToken], start: int = 0, min_precedence: int = 0) -> Dictionary:
	var result = parse_primary(tokens, start)
	if result.node == null:
		return result
	var pos = result.next_pos
	var left = result.node
	
	while pos < tokens.size():
		var token = tokens[pos]
		if token.type != "OPERATOR" or get_precedence(token) < min_precedence:
			break
			
		var op = token
		var precedence = get_precedence(op)
		pos += 1
		
		if pos >= tokens.size():
			push_error("Unexpected end of expression")
			return {"node": null, "next_pos": pos}
		
		var right_result = parse_expression(tokens, pos, precedence + 1)
		if right_result.node == null:
			return right_result
		pos = right_result.next_pos
		
		left = create_binary_node(op.value, left, right_result.node)
	
	return {"node": left, "next_pos": pos}

func parse_primary(tokens: Array[EquationToken], start: int) -> Dictionary:
	if start >= tokens.size():
		push_error("Unexpected end of expression")
		return {"node": null, "next_pos": start}
	
	var pos = start + 1
	var token = tokens[start]
	
	match token.type:
		"NUMBER":
			var node = ImprovedASTNode.new("NUMBER", token.value)
			return {"node": node, "next_pos": pos}
		
		"IDENTIFIER":
			if pos < tokens.size() and tokens[pos].type == "LPAREN":
				# Function call
				pos += 1  # Skip LPAREN
				var args: Array[ImprovedASTNode] = []
				
				while pos < tokens.size() and tokens[pos].type != "RPAREN":
					var arg_result = parse_expression(tokens, pos)
					if arg_result.node == null:
						return {"node": null, "next_pos": pos}
					args.append(arg_result.node)
					pos = arg_result.next_pos
					
					if pos < tokens.size() and tokens[pos].type == "COMMA":
						pos += 1
				
				if pos >= tokens.size() or tokens[pos].type != "RPAREN":
					push_error("Expected closing parenthesis")
					return {"node": null, "next_pos": pos}
				pos += 1  # Skip RPAREN
				
				var node = ImprovedASTNode.new("FUNCTION_CALL", token.value)
				node.arguments = args
				
				# Propagate dependencies from arguments
				for arg in args:
					if arg != null:
						for dep in arg.get_dependencies():
							node.add_dependency(dep)
				
				if token.value == "der":
					node.is_differential = true
					if args.size() > 0 and args[0] != null:
						# For derivatives, we need to track both the full variable name and its dependencies
						var state_var = args[0].dependencies[0] if args[0].dependencies.size() > 0 else args[0].value
						node.state_variable = state_var
						node.add_dependency(state_var)
				
				return {"node": node, "next_pos": pos}
			else:
				# Variable or component reference
				var node = ImprovedASTNode.new("VARIABLE", token.value)
				node.add_dependency(token.value)
				
				# Check for dot notation
				if pos < tokens.size() and tokens[pos].type == "DOT":
					pos += 1  # Skip DOT
					if pos >= tokens.size() or tokens[pos].type != "IDENTIFIER":
						push_error("Expected identifier after dot")
						return {"node": null, "next_pos": pos}
					
					# Create a component reference node
					var comp_node = node  # The component name
					node = ImprovedASTNode.new("VARIABLE", tokens[pos].value)
					node.add_dependency(comp_node.value + "." + tokens[pos].value)
					pos += 1  # Skip the member name
				
				return {"node": node, "next_pos": pos}
		
		"OPERATOR":
			if token.value in ["+", "-"] and (start == 0 or tokens[start-1].type in ["OPERATOR", "LPAREN"]):
				# Unary operator
				var operand_result = parse_primary(tokens, pos)
				pos = operand_result.next_pos
				
				var node = ImprovedASTNode.new("UNARY_OP", token.value)
				node.operand = operand_result.node
				
				# Propagate dependencies from operand
				if operand_result.node != null:
					for dep in operand_result.node.get_dependencies():
						node.add_dependency(dep)
				
				return {"node": node, "next_pos": pos}
		
		"LPAREN":
			var inner_result = parse_expression(tokens, pos)
			pos = inner_result.next_pos
			
			if pos >= tokens.size() or tokens[pos].type != "RPAREN":
				push_error("Expected closing parenthesis")
				return {"node": null, "next_pos": pos}
			pos += 1  # Skip RPAREN
			
			return {"node": inner_result.node, "next_pos": pos}
	
	push_error("Unexpected token: " + str(token))
	return {"node": null, "next_pos": pos}

func create_binary_node(op: String, left: ImprovedASTNode, right: ImprovedASTNode) -> ImprovedASTNode:
	if left == null or right == null:
		return null
		
	var node = ImprovedASTNode.new("BINARY_OP", op)
	node.left = left
	node.right = right
	
	# Propagate dependencies from children
	for dep in left.get_dependencies():
		node.add_dependency(dep)
	for dep in right.get_dependencies():
		node.add_dependency(dep)
	
	return node

func get_precedence(token: EquationToken) -> int:
	match token.value:
		"+", "-": return 1
		"*", "/": return 2
		"^": return 3
		_: return 0

func is_unary_operator(op: String) -> bool:
	return op in ["+", "-"]

func _evaluate_expression(expression: String, component: ModelicaComponent, state: Dictionary) -> float:
	print("Evaluating expression: " + expression)
	var tokens = tokenize(expression)
	print("Tokens: " + str(tokens))
	var ast_dict = parse_expression(tokens)
	print("AST: " + str(ast_dict))
	
	# The AST node is already an ASTNode object, no need to convert
	return evaluate_ast(ast_dict.node, component, state)

func evaluate_ast(node: ImprovedASTNode, component: ModelicaComponent, variables: Dictionary) -> float:
	if node == null:
		return 0.0
		
	match node.type:
		"NUMBER":
			return float(node.value)
			
		"VARIABLE":
			# Get the full variable name from dependencies
			var var_name = node.dependencies[0] if node.dependencies.size() > 0 else node.value
			
			if "." in var_name:
				# Handle component references (e.g., mass1.position)
				var parts = var_name.split(".")
				var comp_name = parts[0]
				var var_name_only = parts[1]
				
				# First try to get from variables dictionary
				if variables.has(var_name):
					return variables[var_name]
				
				# Then try to find the referenced component
				for comp in components:
					if comp.component_name == comp_name:
						# Try parameter first, then variable
						var param_value = comp.get_parameter(var_name_only)
						if param_value != null:
							return param_value
						return comp.get_variable(var_name_only)
				
				push_error("Component not found: " + comp_name)
				return 0.0
			else:
				# First try to get from variables dictionary
				if variables.has(var_name):
					return variables[var_name]
				
				# Then try parameter first, then variable from the component
				var param_value = component.get_parameter(var_name)
				if param_value != null:
					return param_value
				return component.get_variable(var_name)
				
		"BINARY_OP":
			var left_val = evaluate_ast(node.left, component, variables)
			var right_val = evaluate_ast(node.right, component, variables)
			
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
			var operand_val = evaluate_ast(node.operand, component, variables)
			match node.value:
				"+": return operand_val
				"-": return -operand_val
				_:
					push_error("Unknown unary operator: " + node.value)
					return 0.0
					
		"FUNCTION_CALL":
			var args = []
			for arg in node.arguments:
				args.append(evaluate_ast(arg, component, variables))
				
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
		return expr.substr(start_idx, end_idx - start_idx)
	return "" 

