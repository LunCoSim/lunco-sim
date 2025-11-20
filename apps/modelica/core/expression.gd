class_name ModelicaExpression
extends RefCounted

# Types of expressions
enum ExpressionType {
	VARIABLE,   # A variable reference (x, y, etc.)
	CONSTANT,   # A literal constant (1.0, 3.14, etc.)
	OPERATOR,   # An operator (+, -, *, /, etc.)
	FUNCTION,   # A function call (sin, cos, etc.)
	DERIVATIVE  # A derivative expression (der(x))
}

# The type of expression
var type: int = ExpressionType.CONSTANT

# The value of the expression (depends on type)
# For VARIABLE: the variable name
# For CONSTANT: the numeric value
# For OPERATOR: the operator string ("+", "-", etc.)
# For FUNCTION: the function name ("sin", "cos", etc.)
# For DERIVATIVE: "der"
var value = null

# Arguments to this expression (for operators and functions)
var arguments = []

# Constructor
func _init(p_type: int, p_value, p_arguments: Array = []):
	type = p_type
	value = p_value
	arguments = p_arguments

# Evaluate this expression with the given variable values
func evaluate(variable_values: Dictionary) -> float:
	match type:
		ExpressionType.CONSTANT:
			return value
		
		ExpressionType.VARIABLE:
			if value in variable_values:
				return variable_values[value]
			else:
				push_error("Variable not found in values: " + str(value))
				return 0.0
		
		ExpressionType.OPERATOR:
			return _evaluate_operator(variable_values)
		
		ExpressionType.FUNCTION:
			return _evaluate_function(variable_values)
		
		ExpressionType.DERIVATIVE:
			push_error("Cannot directly evaluate a derivative expression")
			return 0.0
	
	push_error("Unknown expression type: " + str(type))
	return 0.0

# Helper to evaluate an operator expression
func _evaluate_operator(variable_values: Dictionary) -> float:
	# Binary operators
	if arguments.size() == 2:
		var left = arguments[0].evaluate(variable_values)
		var right = arguments[1].evaluate(variable_values)
		
		match value:
			"+": return left + right
			"-": return left - right
			"*": return left * right
			"/": 
				if right == 0:
					push_error("Division by zero")
					return 0.0
				return left / right
			"^": return pow(left, right)
			"==": return 1.0 if left == right else 0.0
			"!=": return 1.0 if left != right else 0.0
			"<": return 1.0 if left < right else 0.0
			">": return 1.0 if left > right else 0.0
			"<=": return 1.0 if left <= right else 0.0
			">=": return 1.0 if left >= right else 0.0
	
	# Unary operators
	if arguments.size() == 1:
		var arg = arguments[0].evaluate(variable_values)
		
		match value:
			"-": return -arg
			"+": return arg
	
	push_error("Invalid operator or argument count: " + str(value) + " with " + str(arguments.size()) + " arguments")
	return 0.0

# Helper to evaluate a function expression
func _evaluate_function(variable_values: Dictionary) -> float:
	var args = []
	for arg in arguments:
		args.append(arg.evaluate(variable_values))
	
	match value:
		"sin": 
			if args.size() != 1:
				push_error("sin() requires exactly 1 argument")
				return 0.0
			return sin(args[0])
		
		"cos": 
			if args.size() != 1:
				push_error("cos() requires exactly 1 argument")
				return 0.0
			return cos(args[0])
		
		"tan": 
			if args.size() != 1:
				push_error("tan() requires exactly 1 argument")
				return 0.0
			return tan(args[0])
		
		"sqrt": 
			if args.size() != 1:
				push_error("sqrt() requires exactly 1 argument")
				return 0.0
			if args[0] < 0:
				push_error("sqrt() called with negative argument")
				return 0.0
			return sqrt(args[0])
		
		"exp": 
			if args.size() != 1:
				push_error("exp() requires exactly 1 argument")
				return 0.0
			return exp(args[0])
		
		"log", "ln": 
			if args.size() != 1:
				push_error("log() requires exactly 1 argument")
				return 0.0
			if args[0] <= 0:
				push_error("log() called with non-positive argument")
				return 0.0
			return log(args[0])
		
		"abs": 
			if args.size() != 1:
				push_error("abs() requires exactly 1 argument")
				return 0.0
			return abs(args[0])
		
		"min": 
			if args.size() < 2:
				push_error("min() requires at least 2 arguments")
				return 0.0
			return args.min()
		
		"max": 
			if args.size() < 2:
				push_error("max() requires at least 2 arguments")
				return 0.0
			return args.max()
	
	push_error("Unknown function: " + str(value))
	return 0.0

# String representation for debugging
func _to_string() -> String:
	match type:
		ExpressionType.CONSTANT:
			return str(value)
		
		ExpressionType.VARIABLE:
			return str(value)
		
		ExpressionType.OPERATOR:
			if arguments.size() == 1:
				return "(%s%s)" % [value, str(arguments[0])]
			elif arguments.size() == 2:
				return "(%s %s %s)" % [str(arguments[0]), value, str(arguments[1])]
			else:
				return "%s(%s)" % [value, ", ".join(arguments.map(func(arg): return str(arg)))]
		
		ExpressionType.FUNCTION:
			var args_str = ", ".join(arguments.map(func(arg): return str(arg)))
			return "%s(%s)" % [value, args_str]
		
		ExpressionType.DERIVATIVE:
			if arguments.size() > 0:
				return "der(%s)" % str(arguments[0])
			else:
				return "der(?)"
	
	return "UnknownExpression"

# Static helpers to create different types of expressions
static func create_constant(constant_value: float) -> ModelicaExpression:
	return ModelicaExpression.new(ExpressionType.CONSTANT, constant_value)

static func create_variable(var_name: String) -> ModelicaExpression:
	return ModelicaExpression.new(ExpressionType.VARIABLE, var_name)

static func create_operator(op: String, args: Array) -> ModelicaExpression:
	return ModelicaExpression.new(ExpressionType.OPERATOR, op, args)

static func create_function(func_name: String, args: Array) -> ModelicaExpression:
	return ModelicaExpression.new(ExpressionType.FUNCTION, func_name, args)

static func create_derivative(var_name: String) -> ModelicaExpression:
	var var_expr = create_variable(var_name)
	return ModelicaExpression.new(ExpressionType.DERIVATIVE, "der", [var_expr]) 
