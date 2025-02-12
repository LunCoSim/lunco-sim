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

func _init() -> void:
	equations = []
	variables = {}
	derivatives = {}
	components = []
	initial_conditions = []
	time = 0.0

func _ready() -> void:
	print("EquationSystem: Ready")

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
	# Solve one time step using implicit Euler method
	# 1. Predict state variables
	var predicted_state = variables.duplicate()
	for var_name in derivatives:
		predicted_state[var_name] = variables[var_name] + dt * derivatives[var_name]
	
	# 2. Solve the nonlinear system at the new time
	var converged = false
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		
		# Update variables based on equations
		for eq in equations:
			if eq.is_differential:
				# Handle differential equations
				var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
				var rhs_value = _evaluate_expression(eq.right, eq.component, predicted_state)
				derivatives[der_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - derivatives.get(der_var, 0.0)))
			else:
				# Handle algebraic equations
				var rhs_value = _evaluate_expression(eq.right, eq.component, predicted_state)
				var lhs_var = eq.left
				predicted_state[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - variables.get(lhs_var, 0.0)))
		
		if max_residual < TOLERANCE:
			converged = true
			break
	
	if not converged:
		push_warning("Solver did not converge in " + str(MAX_ITERATIONS) + " iterations")
	
	# 3. Update state
	variables = predicted_state
	time += dt

func initialize() -> void:
	# Apply initial conditions
	for ic in initial_conditions:
		variables[ic.variable] = ic.value
	
	# Solve initial system (algebraic equations only)
	for iter in range(MAX_ITERATIONS):
		var max_residual = 0.0
		
		for eq in equations:
			if not eq.is_differential:
				var rhs_value = _evaluate_expression(eq.right, eq.component, variables)
				var lhs_var = eq.left
				var old_value = variables.get(lhs_var, 0.0)
				variables[lhs_var] = rhs_value
				max_residual = max(max_residual, abs(rhs_value - old_value))
		
		if max_residual < TOLERANCE:
			break

func _evaluate_expression(expr: String, component: ModelicaComponent, state: Dictionary) -> float:
	# Enhanced expression evaluator
	if expr.is_valid_float():
		return float(expr)
	
	# Handle der() expressions
	if expr.begins_with("der("):
		var var_name = _extract_der_variable(expr)
		return derivatives.get(var_name, 0.0)
	
	# Handle component parameters and variables
	if expr.contains("."):
		var parts = expr.split(".")
		var comp_name = parts[0]
		var var_name = parts[1]
		
		# Find the component
		for comp in components:
			if comp.name == comp_name:
				if var_name in comp.parameters:
					return comp.parameters[var_name]
				else:
					return state.get(comp_name + "." + var_name, 0.0)
	
	# Handle basic arithmetic
	# This is a simplified version - would need a proper expression parser
	var value = 0.0
	if "+" in expr:
		var parts = expr.split("+")
		value = _evaluate_expression(parts[0].strip_edges(), component, state)
		value += _evaluate_expression(parts[1].strip_edges(), component, state)
	elif "-" in expr:
		var parts = expr.split("-")
		value = _evaluate_expression(parts[0].strip_edges(), component, state)
		value -= _evaluate_expression(parts[1].strip_edges(), component, state)
	elif "*" in expr:
		var parts = expr.split("*")
		value = _evaluate_expression(parts[0].strip_edges(), component, state)
		value *= _evaluate_expression(parts[1].strip_edges(), component, state)
	elif "/" in expr:
		var parts = expr.split("/")
		value = _evaluate_expression(parts[0].strip_edges(), component, state)
		var denominator = _evaluate_expression(parts[1].strip_edges(), component, state)
		if abs(denominator) > 1e-10:
			value /= denominator
		else:
			push_error("Division by zero in expression: " + expr)
	else:
		value = state.get(expr, 0.0)
	
	return value

func _extract_der_variable(expr: String) -> String:
	# Extract variable name from der(variable)
	var start = expr.find("(") + 1
	var end = expr.find(")")
	return expr.substr(start, end - start)

func clear() -> void:
	equations.clear()
	variables.clear()
	derivatives.clear()
	components.clear()
	initial_conditions.clear()
	time = 0.0 
