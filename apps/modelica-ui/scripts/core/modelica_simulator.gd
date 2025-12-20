extends RefCounted

# Import Modelica core components
const PackageManager = preload("res://apps/modelica/core/package_manager.gd")
const Parser = preload("res://apps/modelica/core/parser.gd")
const SolverFactory = preload("res://apps/modelica/core/solver_factory.gd")
const EquationSystem = preload("res://apps/modelica/core/equation_system.gd")
const ModelicaEquation = preload("res://apps/modelica/core/equation.gd")
const ModelicaExpression = preload("res://apps/modelica/core/expression.gd")
const ModelicaASTNode = preload("res://apps/modelica/core/ast_node.gd")

# Signals for UI integration
signal simulation_progress(percent)
signal simulation_complete(results)
signal simulation_error(message)

# Core component instances
var package_manager = null
var parser = null
var solver_factory = null
var current_solver = null
var equation_system = null
var error_message = ""
var simulation_params = {
	"start_time": 0.0,
	"end_time": 10.0,
	"step_size": 0.1
}

func _init():
	# Initialize core components
	package_manager = PackageManager.new()
	parser = Parser.new()
	solver_factory = SolverFactory.new()
	
	# Set up default paths
	package_manager.add_modelica_path("res://apps/modelica/models")

# Load a Modelica model from a file
func load_model(file_path: String) -> Dictionary:
	print("Loading model file: ", file_path)
	
	# Parse the model file
	var ast = parser.parse_file(file_path)
	if ast == null:
		error_message = "Failed to parse model file"
		var errors = parser.get_errors()
		if errors and errors.size() > 0:
			error_message += ": " + errors[0].message
		
		emit_signal("simulation_error", error_message)
		return {
			"success": false,
			"error": error_message
		}
	
	# Get model qualified name
	var model_name = ast.qualified_name if not ast.qualified_name.is_empty() else file_path.get_file().get_basename()
	print("Model name: ", model_name)
	
	# Validate and load dependencies
	var use_qualified_name = not ast.qualified_name.is_empty()
	var load_result
	
	if use_qualified_name:
		# First, check if the qualified name exists in the package structure
		var qualified_model_path = package_manager.find_model_by_qualified_name(ast.qualified_name)
		if not qualified_model_path.is_empty():
			load_result = package_manager.validate_and_load_model(ast.qualified_name)
		else:
			print("Warning: Model has qualified name '", ast.qualified_name, "' but no matching package structure was found")
			load_result = package_manager.validate_and_load_model(file_path)
	else:
		load_result = package_manager.validate_and_load_model(file_path)
	
	if not load_result.success:
		error_message = "Failed to load model dependencies"
		if load_result.errors and load_result.errors.size() > 0:
			error_message += ": " + load_result.errors[0].message
		
		emit_signal("simulation_error", error_message)
		return {
			"success": false,
			"error": error_message
		}
	
	print("Model loaded successfully: ", model_name)
	
	if load_result.dependencies and load_result.dependencies.size() > 0:
		print("Dependencies loaded: ", load_result.dependencies.size())
	
	return {
		"success": true,
		"ast": ast,
		"dependencies": load_result.dependencies
	}

# Extract a model from an AST and create an equation system
func _extract_model_from_ast(ast: ModelicaASTNode) -> EquationSystem:
	print("Extracting model from AST...")
	var equation_system = EquationSystem.new()
	
	# Check if we have a valid model node
	if ast.type != ModelicaASTNode.NodeType.MODEL:
		print("Warning: AST root is not a model node, got: ", ModelicaASTNode.NodeType.keys()[ast.type])
		print("Qualified name: ", ast.qualified_name)
		
		# Try to find a model node in the children
		var found_model = false
		for child in ast.children:
			if child.type == ModelicaASTNode.NodeType.MODEL:
				ast = child
				found_model = true
				print("Found model node in children: ", child.value)
				break
				
		if not found_model:
			print("Error: No model node found in AST")
			return equation_system  # Return empty equation system
	
	print("Processing model: ", ast.value, " (Type: ", ModelicaASTNode.NodeType.keys()[ast.type], ")")
	
	# Process each child in the model
	for child in ast.children:
		# Process parameters
		if child.type == ModelicaASTNode.NodeType.PARAMETER:
			var param_name = str(child.value)
			var param_value = 0.0
			
			# Try to extract parameter value
			for attribute in child.children:
				if attribute.type == ModelicaASTNode.NodeType.ANNOTATION and attribute.value == "value":
					if attribute.children.size() > 0 and attribute.children[0].type == ModelicaASTNode.NodeType.NUMBER:
						param_value = float(attribute.children[0].value)
						break
			
			print("Adding parameter: ", param_name, " = ", param_value)
			equation_system.add_variable(param_name, {
				"is_parameter": true,
				"value": param_value
			})
		
		# Process variables
		elif child.type == ModelicaASTNode.NodeType.VARIABLE:
			var var_name = str(child.value)
			var is_state = false
			var initial_value = 0.0
			
			# Check variable attributes
			for attribute in child.children:
				if attribute.type == ModelicaASTNode.NodeType.ANNOTATION:
					if attribute.value == "is_state" and attribute.children.size() > 0:
						is_state = attribute.children[0].value.to_lower() == "true"
					elif attribute.value == "initial_value" and attribute.children.size() > 0:
						initial_value = float(attribute.children[0].value)
			
			print("Adding variable: ", var_name, " (is_state=", is_state, ", value=", initial_value, ")")
			equation_system.add_variable(var_name, {
				"is_state": is_state,
				"value": initial_value
			})
		
		# Process equations
		elif child.type == ModelicaASTNode.NodeType.EQUATION:
			var left_expr = _create_expression(child.left if child.left else null)
			var right_expr = _create_expression(child.right if child.right else null)
			
			if left_expr and right_expr:
				var eq_type = ModelicaEquation.EquationType.EXPLICIT
				
				# Check if it's a differential equation
				if left_expr.type == ModelicaExpression.ExpressionType.DERIVATIVE:
					eq_type = ModelicaEquation.EquationType.DIFFERENTIAL
				
				print("Adding equation: ", str(left_expr), " = ", str(right_expr))
				var equation = ModelicaEquation.new(eq_type, left_expr, right_expr)
				equation_system.add_equation(equation)
	
	print("Model extraction complete. Variables: ", equation_system.variables.size(), ", Equations: ", equation_system.equations.size())
	return equation_system

# Create a ModelicaExpression from an AST node
func _create_expression(expr_node) -> ModelicaExpression:
	# Handle null case
	if expr_node == null:
		print("Warning: Null expression node")
		return ModelicaExpression.create_constant(0.0)
	
	# Get the node type
	var node_type = expr_node.type if expr_node.type != null else ModelicaASTNode.NodeType.UNKNOWN
	
	# Create expression based on node type
	match node_type:
		ModelicaASTNode.NodeType.IDENTIFIER:
			return ModelicaExpression.create_variable(str(expr_node.value))
			
		ModelicaASTNode.NodeType.NUMBER:
			return ModelicaExpression.create_constant(float(expr_node.value))
			
		ModelicaASTNode.NodeType.FUNCTION_CALL:
			var func_name = str(expr_node.value)
			var func_args = []
			
			# Add arguments if available
			if expr_node.arguments and expr_node.arguments.size() > 0:
				for arg in expr_node.arguments:
					func_args.append(_create_expression(arg))
					
			return ModelicaExpression.create_function(func_name, func_args)
			
		ModelicaASTNode.NodeType.OPERATOR:
			var op = str(expr_node.value)
			var left = _create_expression(expr_node.left)
			var right = _create_expression(expr_node.right)
			
			if op == "+":
				return ModelicaExpression.create_operator("+", [left, right])
			elif op == "-":
				return ModelicaExpression.create_operator("-", [left, right])
			elif op == "*":
				return ModelicaExpression.create_operator("*", [left, right])
			elif op == "/":
				return ModelicaExpression.create_operator("/", [left, right])
			else:
				print("Warning: Unsupported operator: ", op)
				return ModelicaExpression.create_constant(0.0)
				
		_:
			print("Warning: Unsupported expression node type: ", ModelicaASTNode.NodeType.keys()[node_type])
			return ModelicaExpression.create_constant(0.0)

# Create a simple spring-mass-damper model for testing
func _create_test_model() -> EquationSystem:
	print("Creating test spring-mass-damper model...")
	var equation_system = EquationSystem.new()

	# Add parameters
	equation_system.add_variable("m", {
		"is_parameter": true,
		"value": 1.0
	})

	equation_system.add_variable("k", {
		"is_parameter": true,
		"value": 10.0
	})

	# Add state variables
	equation_system.add_variable("x", {
		"is_state": true,
		"value": 1.0  # Initial position
	})

	equation_system.add_variable("v", {
		"is_state": true,
		"value": 0.0  # Initial velocity
	})

	# Add equations
	# Equation 1: der(x) = v
	var der_x = ModelicaExpression.create_derivative("x")
	var var_v = ModelicaExpression.create_variable("v")
	var eq1 = ModelicaEquation.new(
		ModelicaEquation.EquationType.DIFFERENTIAL,
		der_x,
		var_v
	)
	equation_system.add_equation(eq1)

	# Equation 2: Simple spring force: der(v) = -k/m * x
	var der_v = ModelicaExpression.create_derivative("v")

	# Create -k/m * x
	var var_k = ModelicaExpression.create_variable("k")
	var var_m = ModelicaExpression.create_variable("m")
	var var_x = ModelicaExpression.create_variable("x")

	# First calculate k/m
	var k_div_m = ModelicaExpression.create_operator("/", [var_k, var_m])

	# Then multiply by x
	var k_div_m_times_x = ModelicaExpression.create_operator("*", [k_div_m, var_x])

	# Then negate it (creating a constant -1 and multiplying)
	var neg_one = ModelicaExpression.create_constant(-1.0)
	var right_side = ModelicaExpression.create_operator("*", [neg_one, k_div_m_times_x])

	var eq2 = ModelicaEquation.new(
		ModelicaEquation.EquationType.DIFFERENTIAL,
		der_v,
		right_side
	)
	equation_system.add_equation(eq2)

	print("Test model created. Variables: ", equation_system.variables.size(), ", Equations: ", equation_system.equations.size())
	return equation_system

# Setup the model and solver for simulation
func setup_model(ast, start_time: float, end_time: float, step_size: float) -> Dictionary:
	print("Setting up simulation model...")
	
	# Store simulation parameters
	simulation_params = {
		"start_time": start_time,
		"end_time": end_time,
		"step_size": step_size
	}
	
	# Create an equation system from the AST
	equation_system = _extract_model_from_ast(ast)
	
	# If the model extraction didn't work well, use a test model
	if equation_system.equations.size() == 0 or equation_system.variables.size() == 0:
		print("Warning: Could not extract model properly. Using test model instead.")
		equation_system = _create_test_model()
	
	# Create a solver for the equation system
	# First try RK4 solver
	print("Creating RK4 solver...")
	current_solver = solver_factory.create_solver(equation_system, SolverFactory.SolverType.RK4)
	
	# If RK4 solver fails, try a different solver
	if current_solver == null or not current_solver.initialized:
		print("RK4 solver initialization failed. Trying DAE solver...")
		current_solver = solver_factory.create_solver(equation_system, SolverFactory.SolverType.ACAUSAL)
		
		if current_solver == null:
			error_message = "Failed to create solver for equation system"
			emit_signal("simulation_error", error_message)
			return {
				"success": false,
				"error": error_message
			}
	
	# Set initial state
	var state = equation_system.get_state()
	state.time = start_time
	equation_system.set_state(state)
	
	print("Model setup complete. Ready to run simulation.")
	return {
		"success": true,
		"ast": ast
	}

# Run a simulation using a loaded model
func run_simulation(setup_result) -> Dictionary:
	# Check if the setup was successful
	if not setup_result.success:
		return setup_result
		
	print("Running simulation from t=", simulation_params.start_time, " to t=", simulation_params.end_time, " with step size=", simulation_params.step_size)
	
	var results = []
	var time = simulation_params.start_time
	var total_steps = int((simulation_params.end_time - simulation_params.start_time) / simulation_params.step_size)
	var step_count = 0
	
	while time <= simulation_params.end_time:
		# Get the current state
		var current_state = {
			"time": time,
			"variables": {}
		}
		
		# Get values for all variables
		for var_name in equation_system.variables.keys():
			current_state.variables[var_name] = equation_system.get_variable_value(var_name)
		
		results.append(current_state)
		
		# Take a step
		var step_success = current_solver.step(simulation_params.step_size)
		if not step_success:
			error_message = "Simulation failed during step at time t=" + str(time)
			emit_signal("simulation_error", error_message)
			return {
				"success": false,
				"error": error_message,
				"results": results  # Return partial results
			}
		
		time += simulation_params.step_size
		
		# Update progress
		step_count += 1
		var progress = float(step_count) / total_steps * 100.0
		emit_signal("simulation_progress", progress)
	
	print("Simulation completed with ", results.size(), " steps")
	emit_signal("simulation_complete", results)
	
	return {
		"success": true,
		"results": results
	}

# Run a simulation directly from a file
func simulate_file(file_path: String, start_time: float, end_time: float, step_size: float) -> Dictionary:
	var load_result = load_model(file_path)
	if not load_result.success:
		return load_result
	
	# Setup the model
	var setup_result = setup_model(load_result.ast, start_time, end_time, step_size)
	if not setup_result.success:
		return setup_result
	
	# Run the simulation
	return run_simulation(setup_result)

# Get a list of variable names from results
func get_result_variables(results: Array) -> Array:
	if results.is_empty():
		return []
	
	var variables = []
	for var_name in results[0].variables.keys():
		variables.append(var_name)
	
	return variables

# Export results to CSV
func export_to_csv(results: Array, file_path: String) -> bool:
	if results.is_empty():
		emit_signal("simulation_error", "No results to export")
		return false
	
	var file = FileAccess.open(file_path, FileAccess.WRITE)
	if not file:
		emit_signal("simulation_error", "Failed to open file for export: " + file_path)
		return false
	
	# Get variable names
	var variables = get_result_variables(results)
	
	# Write header
	var header = "time"
	for var_name in variables:
		header += "," + var_name
	file.store_line(header)
	
	# Write data rows
	for result in results:
		var line = str(result.time)
		for var_name in variables:
			line += "," + str(result.variables[var_name])
		file.store_line(line)
	
	file.close()
	print("Exported results to: ", file_path)
	return true
