extends BaseTest

const Parser = preload("res://apps/modelica/core/parser.gd")
const ASTNode = preload("res://apps/modelica/core/ast_node.gd")
const DAESolver = preload("res://apps/modelica/core/solver.gd")

var parser: Parser
var solver: DAESolver

func setup():
	parser = Parser.create_modelica_parser()
	solver = DAESolver.new()

func test_simple_mass_spring_model():
	var model_source = """
	model SimpleSpringMass
		parameter Real m = 1.0 "Mass";
		parameter Real k = 10.0 "Spring constant";
		parameter Real x0 = 1.0 "Initial position";
		parameter Real v0 = 0.0 "Initial velocity";
		Real x "Position";
		Real v "Velocity";
	initial equation
		x = x0;
		v = v0;
	equation
		v = der(x);
		m * der(v) + k * x = 0;
	end SimpleSpringMass;
	"""
	
	# Parse the model
	var ast = parser.parse(model_source)
	assert_not_null(ast, "AST should not be null")
	assert_equal(ast.type, ASTNode.NodeType.MODEL, "Root node should be a model")
	assert_equal(ast.name, "SimpleSpringMass", "Model name should be 'SimpleSpringMass'")
	
	# Set up equation system based on the model
	solver = setup_equation_system(ast)
	assert_not_null(solver, "Solver should be set up")
	
	# Run simulation for 1 second with a 0.1 time step
	var results = []
	var t = 0.0
	var dt = 0.1
	var end_time = 1.0
	
	# Store initial state
	results.append({
		"time": t,
		"x": solver.get_variable_value("x"),
		"v": solver.get_variable_value("v")
	})
	
	# Run simulation
	while t < end_time:
		# Update derivatives based on current state
		update_derivatives(solver)
		
		# Take a step
		var success = solver.step(dt)
		assert_true(success, "Simulation step should succeed")
		
		# Advance time
		t += dt
		
		# Store results
		results.append({
			"time": t,
			"x": solver.get_variable_value("x"),
			"v": solver.get_variable_value("v")
		})
	}
	
	# Check results against analytical solution for spring-mass system
	# x(t) = x0 * cos(ω * t), where ω = sqrt(k/m)
	var omega = sqrt(solver.get_variable_value("k") / solver.get_variable_value("m"))
	
	for result in results:
		var t_val = result.time
		var x_val = result.x
		var expected_x = solver.get_variable_value("x0") * cos(omega * t_val)
		
		# Allow some error due to numerical integration
		assert_almost_equal(x_val, expected_x, 0.1, "Position should match analytical solution at t=" + str(t_val))
	}

# Helper to create a solver from a model AST
func setup_equation_system(ast: ASTNode) -> DAESolver:
	var solver = DAESolver.new()
	
	# Add parameters and variables
	for node in ast.children:
		if node.type == ASTNode.NodeType.PARAMETER:
			solver.add_parameter(node.name, node.value)
		elif node.type == ASTNode.NodeType.VARIABLE:
			if node.variability == "continuous":
				solver.add_state_variable(node.name, 0.0)
	
	# Handle initial equations
	for node in ast.children:
		if node.type == ASTNode.NodeType.INITIAL_EQUATION:
			for eq in node.children:
				if eq.type == ASTNode.NodeType.EQUATION and eq.operator == "=":
					if eq.left.type == ASTNode.NodeType.VARIABLE_REF:
						var var_name = eq.left.name
						var value = evaluate_expression(eq.right, solver)
						solver.set_variable_value(var_name, value)
	
	# Add equations
	for node in ast.children:
		if node.type == ASTNode.NodeType.EQUATION:
			for eq in node.children:
				if eq.type == ASTNode.NodeType.EQUATION:
					solver.add_equation(eq.str())
	
	return solver

# Helper to evaluate an expression with current variable values
func evaluate_expression(expr: ASTNode, solver: DAESolver):
	match expr.type:
		ASTNode.NodeType.LITERAL:
			return expr.value
		
		ASTNode.NodeType.VARIABLE_REF:
			return solver.get_variable_value(expr.name)
		
		ASTNode.NodeType.BINARY_OP:
			var left_val = evaluate_expression(expr.left, solver)
			var right_val = evaluate_expression(expr.right, solver)
			
			match expr.operator:
				"+": return left_val + right_val
				"-": return left_val - right_val
				"*": return left_val * right_val
				"/": return left_val / right_val
				
		_:
			push_error("Unsupported expression type: " + str(expr.type))
			return 0.0

# Helper to update derivatives based on equations
func update_derivatives(solver: DAESolver):
	# For spring-mass system: v = der(x), der(v) = -k/m * x
	var x = solver.get_variable_value("x")
	var v = solver.get_variable_value("v")
	var k = solver.get_variable_value("k")
	var m = solver.get_variable_value("m")
	
	# Set derivatives
	solver.state_variables["x"].derivative = v
	solver.state_variables["v"].derivative = -k/m * x 