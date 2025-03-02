class_name ModelicaEquation
extends RefCounted

# Types of equations
enum EquationType {
	EXPLICIT,    # x = f(...)
	IMPLICIT,    # f(...) = g(...)
	DIFFERENTIAL # der(x) = f(...)
}

# The type of equation
var type: int = EquationType.EXPLICIT

# Left and right expressions
var left_expression = null
var right_expression = null

# Additional metadata about this equation
var metadata = {}

# Initialization
func _init(eq_type: int, left_expr, right_expr):
	type = eq_type
	left_expression = left_expr
	right_expression = right_expr

# Evaluate this equation and return the residual (difference between left and right sides)
func evaluate(variable_values: Dictionary) -> float:
	var left_value = left_expression.evaluate(variable_values)
	var right_value = right_expression.evaluate(variable_values)
	return left_value - right_value

# Solve for a specific variable using a numerical method
func solve_for(variable_name: String, variable_values: Dictionary) -> float:
	# For explicit equations where left side is the variable, just evaluate the right side
	if type == EquationType.EXPLICIT and left_expression.type == ModelicaExpression.ExpressionType.VARIABLE and left_expression.value == variable_name:
		return right_expression.evaluate(variable_values)
	
	# For explicit equations where right side is the variable, just evaluate the left side
	if type == EquationType.EXPLICIT and right_expression.type == ModelicaExpression.ExpressionType.VARIABLE and right_expression.value == variable_name:
		return left_expression.evaluate(variable_values)
	
	# For more complex cases, use a numerical method
	# Simple bisection method as a fallback
	var x_min = -1000.0
	var x_max = 1000.0
	var max_iterations = 50
	var tolerance = 1e-6
	
	# Clone the variable values so we can modify them
	var values = variable_values.duplicate()
	
	# Evaluate the function at the bounds
	values[variable_name] = x_min
	var f_min = evaluate(values)
	
	values[variable_name] = x_max
	var f_max = evaluate(values)
	
	# Check if there's a root in the interval
	if f_min * f_max > 0:
		push_warning("No root found in interval for variable " + variable_name)
		return 0.0
	
	# Bisection iteration
	for i in range(max_iterations):
		var x_mid = (x_min + x_max) / 2.0
		values[variable_name] = x_mid
		var f_mid = evaluate(values)
		
		if abs(f_mid) < tolerance:
			return x_mid
		
		if f_mid * f_min < 0:
			x_max = x_mid
			f_max = f_mid
		else:
			x_min = x_mid
			f_min = f_mid
	
	# Return the best approximation found
	return (x_min + x_max) / 2.0

# Get all variables involved in this equation
func get_involved_variables() -> Array:
	var variables = []
	_extract_variables_from_expression(left_expression, variables)
	_extract_variables_from_expression(right_expression, variables)
	
	# Remove duplicates
	var unique_vars = []
	for var_name in variables:
		if not var_name in unique_vars:
			unique_vars.append(var_name)
	
	return unique_vars

# Helper to extract variables from an expression
func _extract_variables_from_expression(expr, variables: Array) -> void:
	if expr == null:
		return
	
	if expr.type == ModelicaExpression.ExpressionType.VARIABLE:
		variables.append(expr.value)
	elif expr.type == ModelicaExpression.ExpressionType.DERIVATIVE:
		# For a derivative, extract the variable being differentiated
		if expr.arguments.size() > 0:
			_extract_variables_from_expression(expr.arguments[0], variables)
	
	# Recursively extract variables from arguments
	for arg in expr.arguments:
		_extract_variables_from_expression(arg, variables)

# String representation for debugging
func _to_string() -> String:
	var type_str = "UNKNOWN"
	if type == EquationType.EXPLICIT:
		type_str = "EXPLICIT"
	elif type == EquationType.IMPLICIT:
		type_str = "IMPLICIT"
	elif type == EquationType.DIFFERENTIAL:
		type_str = "DIFFERENTIAL"
	
	return "%s: %s = %s" % [type_str, str(left_expression), str(right_expression)] 