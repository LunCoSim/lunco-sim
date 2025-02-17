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

class ASTNode:
	var type: String
	var value: String
	var left: ASTNode
	var right: ASTNode
	var operand: ASTNode
	var arguments: Array[ASTNode]
	
	func _init(p_type: String, p_value: String = ""):
		type = p_type
		value = p_value
		arguments = []
	
	func _to_string() -> String:
		match type:
			"NUMBER":
				return value
			"BINARY_OP":
				return "(%s %s %s)" % [left, value, right]
			"UNARY_OP":
				return "(%s%s)" % [value, operand]
			"VARIABLE":
				return value
			"FUNCTION_CALL":
				var args_str = ""
				for arg in arguments:
					if args_str:
						args_str += ", "
					args_str += str(arg)
				return "%s(%s)" % [value, args_str]
		return "Node(%s, %s)" % [type, value]

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
	if not result:
		return {}
	
	var pos = result.next_pos
	var left = result.node
	
	while pos < tokens.size():
		var op_token = tokens[pos]
		if op_token.type != "OPERATOR":
			break
			
		var precedence = get_operator_precedence(op_token.value)
		if precedence < min_precedence:
			break
			
		pos += 1
		var right_result = parse_expression(tokens, pos, precedence + 1)
		if not right_result:
			return {}
			
		var node = ASTNode.new("BINARY_OP", op_token.value)
		node.left = left
		node.right = right_result.node
		
		left = node
		pos = right_result.next_pos
	
	return {"node": left, "next_pos": pos}

func parse_primary(tokens: Array[EquationToken], pos: int) -> Dictionary:
	if pos >= tokens.size():
		return {}
	
	var token = tokens[pos]
	match token.type:
		"NUMBER":
			var node = ASTNode.new("NUMBER", token.value)
			return {"node": node, "next_pos": pos + 1}
		"OPERATOR":
			# Handle unary operators
			if token.value in ["+", "-"] and pos + 1 < tokens.size():
				var result = parse_primary(tokens, pos + 1)
				if not result:
					return {}
				var node = ASTNode.new("UNARY_OP", token.value)
				node.operand = result.node
				return {"node": node, "next_pos": result.next_pos}
		"IDENTIFIER":
			if pos + 1 < tokens.size() and tokens[pos + 1].type == "LPAREN":
				# Function call
				var result = parse_function_call(tokens, pos)
				if not result:
					return {}
				return result
			else:
				# Variable or dotted identifier
				var node = ASTNode.new("VARIABLE", token.value)
				var next_pos = pos + 1
				
				# Check for dot notation (e.g., spring.length)
				if next_pos + 1 < tokens.size() and tokens[next_pos].type == "DOT":
					node.value += "." + tokens[next_pos + 1].value
					next_pos += 2
				
				return {"node": node, "next_pos": next_pos}
		"LPAREN":
			var result = parse_expression(tokens, pos + 1)
			if not result or result.next_pos >= tokens.size() or tokens[result.next_pos].type != "RPAREN":
				return {}
			return {"node": result.node, "next_pos": result.next_pos + 1}
	return {}

func parse_function_call(tokens: Array[EquationToken], pos: int) -> Dictionary:
	var func_name = tokens[pos].value
	pos += 2  # Skip function name and left paren
	
	var node = ASTNode.new("FUNCTION_CALL", func_name)
	
	while pos < tokens.size() and tokens[pos].type != "RPAREN":
		var arg_result = parse_expression(tokens, pos)
		if not arg_result:
			return {}
		
		node.arguments.append(arg_result.node)
		pos = arg_result.next_pos
		
		if pos < tokens.size() and tokens[pos].type == "COMMA":
			pos += 1
	
	if pos >= tokens.size() or tokens[pos].type != "RPAREN":
		return {}
	
	return {"node": node, "next_pos": pos + 1}

func get_operator_precedence(op: String) -> int:
	match op:
		"+", "-": return 1
		"*", "/": return 2
		"^": return 3
	return 0

func _evaluate_expression(expression: String, component: ModelicaComponent, state: Dictionary) -> float:
	print("Evaluating expression: " + expression)
	var tokens = tokenize(expression)
	print("Tokens: " + str(tokens))
	var ast_dict = parse_expression(tokens)
	print("AST: " + str(ast_dict))
	
	# The AST node is already an ASTNode object, no need to convert
	return evaluate_ast(ast_dict.node, component, state)

func evaluate_ast(node: ASTNode, component: ModelicaComponent, state: Dictionary) -> float:
	match node.type:
		"NUMBER":
			return float(node.value)
		"VARIABLE":
			var parts = node.value.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name = parts[1]
				if comp_name == component.component_name:
					return component.get_variable(var_name)
				else:
					for comp in components:
						if comp.component_name == comp_name:
							return comp.get_variable(var_name)
			return state[node.value]
		"BINARY_OP":
			var left_val = evaluate_ast(node.left, component, state)
			var right_val = evaluate_ast(node.right, component, state)
			match node.value:
				"+": return left_val + right_val
				"-": return left_val - right_val
				"*": return left_val * right_val
				"/": return left_val / right_val
				"^": return pow(left_val, right_val)
			push_error("Unknown operator: " + node.value)
			return 0.0
		"UNARY_OP":
			var operand_val = evaluate_ast(node.operand, component, state)
			match node.value:
				"-": return -operand_val
				"+": return operand_val
			push_error("Unknown unary operator: " + node.value)
			return 0.0
		"FUNCTION_CALL":
			var args = []
			for arg in node.arguments:
				args.append(evaluate_ast(arg, component, state))
			match node.value:
				"sin": return sin(args[0])
				"cos": return cos(args[0])
				"sqrt": return sqrt(args[0])
				"abs": return abs(args[0])
				"exp": return exp(args[0])
				"log": 
					if args[0] <= 0:
						push_error("Invalid argument for log: " + str(args[0]))
						return 0.0
					return log(args[0])
				"der":
					var var_name = args[0]
					return derivatives[var_name]
			push_error("Unknown function: " + node.value)
			return 0.0
	push_error("Invalid AST node type: " + node.type)
	return 0.0

func _extract_der_variable(expr: String) -> String:
	# Extract variable name from der() expression
	var start_idx = expr.find("der(") + 4
	var end_idx = expr.find(")", start_idx)
	if start_idx >= 4 and end_idx > start_idx:
		return expr.substr(start_idx, end_idx - start_idx)
	return "" 

