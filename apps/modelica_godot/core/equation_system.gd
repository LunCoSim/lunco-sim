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
const TOLERANCE = 1e-6
class Token:
	var type: String
	var value: String
	var position: int
	
	func _init(p_type: String, p_value: String, p_position: int):
		type = p_type
		value = p_value
		position = p_position
	
	func _to_string() -> String:
		return "Token(%s, '%s', %d)" % [type, value, position]

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

func solve_step() -> void:
	# 1. Store current state
	var current_state = variables.duplicate()
	
	# 2. Calculate all derivatives at the current state
	var current_derivatives = {}
	
	# First evaluate non-differential equations to get current forces, accelerations, etc.
	for eq in equations:
		if not eq.is_differential:
			var rhs_value = _evaluate_expression(eq.right, eq.component, current_state)
			var lhs_var = eq.left
			current_state[lhs_var] = rhs_value
			
			# Update component state
			var parts = lhs_var.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name_only = parts[1]
				for comp in components:
					if comp.component_name == comp_name:
						comp.set_variable(var_name_only, rhs_value)
						break
	
	# Then evaluate all differential equations to get derivatives
	for eq in equations:
		if eq.is_differential:
			var rhs_value = _evaluate_expression(eq.right, eq.component, current_state)
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			current_derivatives[der_var] = rhs_value
			
			# Update component derivatives
			var parts = der_var.split(".")
			if parts.size() == 2:
				var comp_name = parts[0]
				var var_name_only = parts[1]
				for comp in components:
					if comp.component_name == comp_name:
						var der_var_name = "der(" + var_name_only + ")"
						if der_var_name in comp.der_variables:
							comp.set_variable(der_var_name, rhs_value)
						break
	
	# 3. Integrate using explicit Euler method
	var next_state = current_state.duplicate()
	
	# Integrate all state variables using their derivatives
	for var_name in current_derivatives:
		var derivative_value = current_derivatives[var_name]
		var current_value = current_state.get(var_name, 0.0)
		var next_value = current_value + dt * derivative_value
		next_state[var_name] = next_value
		
		# Update component state
		var parts = var_name.split(".")
		if parts.size() == 2:
			var comp_name = parts[0]
			var var_name_only = parts[1]
			for comp in components:
				if comp.component_name == comp_name:
					comp.set_variable(var_name_only, next_value)
					break
	
	# 4. Update system state
	variables = next_state
	derivatives = current_derivatives
	
	# 5. Update dependent variables for the next step
	for eq in equations:
		if not eq.is_differential:
			var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
			var lhs_var = eq.left
			variables[lhs_var] = rhs_value
			
			# Update component state
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

func tokenize(expr: String) -> Array[Token]:
	var tokens: Array[Token] = []
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
					tokens.append(Token.new(type, value, pos))
				pos += value.length()
				matched = true
				break
		
		if not matched:
			push_error("Invalid character in expression at position %d: %s" % [pos, expr[pos]])
			return []
	
	return tokens

func parse_expression(tokens: Array[Token], start: int = 0, min_precedence: int = 0) -> Dictionary:
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

func parse_primary(tokens: Array[Token], pos: int) -> Dictionary:
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

func parse_function_call(tokens: Array[Token], pos: int) -> Dictionary:
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

func evaluate_ast(node: ASTNode, component: ModelicaComponent, state: Dictionary) -> float:
	match node.type:
		"NUMBER":
			return float(node.value)
		"BINARY_OP":
			var left_val = evaluate_ast(node.left, component, state)
			var right_val = evaluate_ast(node.right, component, state)
			match node.value:
				"+": return left_val + right_val
				"-": return left_val - right_val
				"*": return left_val * right_val
				"/":
					if abs(right_val) > 1e-10:
						return left_val / right_val
					else:
						push_error("Division by zero")
						return 0.0
				"^": return pow(left_val, right_val)
		"UNARY_OP":
			var val = evaluate_ast(node.operand, component, state)
			match node.value:
				"-": return -val
				"+": return val
		"VARIABLE":
			if node.value.contains("."):
				# Handle dotted identifiers (e.g., spring.length)
				var parts = node.value.split(".")
				var comp_name = parts[0]
				var var_name = parts[1]
				
				# First try state dictionary
				var full_name = node.value
				if full_name in state:
					return state[full_name]
				
				# Then try component
				for comp in components:
					if comp.component_name == comp_name:
						if var_name in comp.parameters:
							return comp.get_parameter(var_name)
						else:
							return comp.get_variable(var_name)
				
				return 0.0
			else:
				# Handle simple variables
				if component != null:
					var full_name = component.component_name + "." + node.value
					if full_name in state:
						return state[full_name]
					elif node.value in component.parameters:
						return component.get_parameter(node.value)
					elif node.value in component.variables:
						return component.get_variable(node.value)
				return state.get(node.value, 0.0)
		"FUNCTION_CALL":
			if node.value == "der":
				if node.arguments.size() == 1:
					# Get the full variable name (e.g., "mass.position")
					var arg = node.arguments[0]
					var var_name = ""
					if arg.type == "VARIABLE":
						var_name = arg.value
						# If the variable doesn't have a component prefix, add it
						if not var_name.contains(".") and component != null:
							var_name = component.component_name + "." + var_name
					else:
						push_error("Invalid argument type for der() function")
						return 0.0
					return derivatives.get(var_name, 0.0)
			elif node.value == "sin":
				if node.arguments.size() == 1:
					return sin(evaluate_ast(node.arguments[0], component, state))
			elif node.value == "cos":
				if node.arguments.size() == 1:
					return cos(evaluate_ast(node.arguments[0], component, state))
			elif node.value == "sqrt":
				if node.arguments.size() == 1:
					var val = evaluate_ast(node.arguments[0], component, state)
					if val >= 0:
						return sqrt(val)
					else:
						push_error("Square root of negative number")
						return 0.0
	push_error("Invalid AST node type: " + node.type)
	return 0.0

func _evaluate_expression(expr: String, component: ModelicaComponent, state: Dictionary) -> float:
	print("Evaluating expression: ", expr)
	var tokens = tokenize(expr)
	print("Tokens: ", tokens)
	
	if tokens.is_empty():
		push_error("Failed to tokenize expression: " + expr)
		return 0.0
	
	var parse_result = parse_expression(tokens)
	if not parse_result or parse_result.next_pos != tokens.size():
		push_error("Failed to parse expression: " + expr)
		return 0.0
	
	var ast = parse_result.node
	print("AST: ", ast)
	
	return evaluate_ast(ast, component, state)

func _extract_der_variable(expr: String) -> String:
	# Extract variable name from der() expression
	var start_idx = expr.find("der(") + 4
	var end_idx = expr.find(")", start_idx)
	if start_idx >= 4 and end_idx > start_idx:
		return expr.substr(start_idx, end_idx - start_idx)
	return "" 

