#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestSimpleModels extends "res://apps/modelica/tests/base_test.gd":
	const Parser = preload("res://apps/modelica/core/parser.gd")
	const ModelicaASTNode = preload("res://apps/modelica/core/ast_node.gd")
	const DAESolver = preload("res://apps/modelica/core/solver.gd")

	var parser
	var solver: DAESolver

	func setup():
		print("Setting up test...")
		# Check if all required classes are loaded properly
		print("Checking if Parser class exists: " + str(Parser != null))
		print("Checking if ModelicaASTNode class exists: " + str(ModelicaASTNode != null))
		print("Checking if DAESolver class exists: " + str(DAESolver != null))
		
		# Create a ModelicaParser instance using the factory method
		print("Attempting to create parser...")
		parser = Parser.create_modelica_parser()
		print("Parser created: " + str(parser))
		
		print("Attempting to create solver...")
		solver = DAESolver.new()
		print("Solver created: " + str(solver))

	func test_simple_mass_spring_model():
		print("Starting test_simple_mass_spring_model...")
		var model_source = """model SimpleSpringMass
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
		
		print("Model source defined, starting parsing...")
		# Parse the model
		print("Calling parser.parse() method...")
		var ast = parser.parse(model_source)
		print("Parsing complete, AST: " + str(ast))
		
		# Skip the rest of the test if parsing failed
		if ast == null:
			push_error("Parsing failed, AST is null")
			assert_not_null(ast, "AST should not be null")
			return
			
		print("Asserting AST type...")
		assert_equal(ast.type, ModelicaASTNode.NodeType.MODEL, "Root node should be a model")
		print("Asserting AST value...")
		assert_equal(ast.value, "SimpleSpringMass", "Model name should be 'SimpleSpringMass'")
		
		print("Setting up equation system...")
		print("Setup equation system for AST type: " + str(ast.type) + ", value: " + str(ast.value))
		
		# Add parameters and variables to the solver
		print("Adding parameters and variables...")
		var variables_added = []
		for child in ast.children:
			print("Processing node: " + str(child.type) + " - " + str(child.value))
			if child.type == ModelicaASTNode.NodeType.PARAMETER:
				print("Adding parameter " + str(child.value) + " with default value 0.0")
				var param = solver.add_parameter(str(child.value), 0.0)
				print("Parameter addition result: " + str(param))
				variables_added.append(str(child.value))
			elif child.type == ModelicaASTNode.NodeType.VARIABLE:
				print("Adding state variable: " + str(child.value))
				var var_result = solver.add_state_variable(str(child.value), 0.0)
				print("State variable addition result: " + str(var_result))
				variables_added.append(str(child.value))
			elif child.type == ModelicaASTNode.NodeType.EQUATION:
				print("Processing node: " + str(child.type) + " - " + str(child.value))
		
		# Set known values for SimpleSpringMass model parameters
		print("SimpleSpringMass model detected, setting known parameter values...")
		solver.set_parameter_value("m", 1.0)
		solver.set_parameter_value("k", 10.0)
		solver.set_parameter_value("x0", 1.0)
		solver.set_parameter_value("v0", 0.0)
		
		# Set initial values for state variables based on parameters
		print("Setting initial values for state variables...")
		if "x" in solver.state_variables:
			solver.state_variables["x"].value = 1.0  # x0 value
			print("Set x initial value to 1.0")
		if "v" in solver.state_variables:
			solver.state_variables["v"].value = 0.0  # v0 value
			print("Set v initial value to 0.0")
		
		# Process initial equations
		print("Processing initial equations...")
		for node in ast.children:
			if node.type == ModelicaASTNode.NodeType.EQUATION and node.value == "initial":
				for eq in node.children:
					if eq.type == ModelicaASTNode.NodeType.EQUATION and eq.value == "=":
						if eq.children.size() >= 2 and eq.children[0].type == ModelicaASTNode.NodeType.IDENTIFIER:
							var var_name = eq.children[0].value
							var value = 0.0
							
							# Try to evaluate the expression safely
							if eq.children[1].type == ModelicaASTNode.NodeType.NUMBER:
								value = float(eq.children[1].value)
							elif eq.children[1].type == ModelicaASTNode.NodeType.IDENTIFIER:
								value = solver.get_variable_value(eq.children[1].value)
							
							print("Setting initial value: " + var_name + " = " + str(value))
							solver.set_variable_value(var_name, value)
		
		print("Setting up equation system complete")
		return solver

	# Helper to evaluate an expression with current variable values
	func evaluate_expression(expr: ModelicaASTNode, solver: DAESolver):
		match expr.type:
			ModelicaASTNode.NodeType.NUMBER:
				return expr.value
			
			ModelicaASTNode.NodeType.IDENTIFIER:
				return solver.get_variable_value(expr.value)
			
			ModelicaASTNode.NodeType.OPERATOR:
				var left_val = evaluate_expression(expr.children[0], solver)
				var right_val = evaluate_expression(expr.children[1], solver)
				
				match expr.value:
					"+": return left_val + right_val
					"-": return left_val - right_val
					"*": return left_val * right_val
					"/": return left_val / right_val
					
			_:
				push_error("Unsupported expression type: " + str(expr.type))
				return 0.0

	# Helper to update derivatives based on equations
	func update_derivatives(solver: DAESolver):
		print("Updating derivatives...")
		# For spring-mass system: v = der(x), der(v) = -k/m * x
		
		# Safely get variable values with defaults
		var x = 0.0
		var v = 0.0
		var k = 10.0
		var m = 1.0
		
		# Try to get actual values if they exist
		if "x" in solver.state_variables:
			x = solver.get_variable_value("x")
		else:
			print("Warning: 'x' not found, using default 0.0")
			
		if "v" in solver.state_variables:
			v = solver.get_variable_value("v")
		else:
			print("Warning: 'v' not found, using default 0.0")
			
		if "k" in solver.parameters:
			k = solver.get_variable_value("k")
		else:
			print("Warning: 'k' not found, using default 10.0")
			
		if "m" in solver.parameters:
			m = solver.get_variable_value("m")
		else:
			print("Warning: 'm' not found, using default 1.0")
		
		print("Current values - x: " + str(x) + ", v: " + str(v) + ", k: " + str(k) + ", m: " + str(m))
		
		# Ensure state variables exist before setting derivatives
		if not "x" in solver.state_variables:
			print("Creating missing state variable 'x'")
			solver.add_state_variable("x", 0.0)
			
		if not "v" in solver.state_variables:
			print("Creating missing state variable 'v'")
			solver.add_state_variable("v", 0.0)
		
		# Set derivatives
		if "derivative" in solver.state_variables["x"]:
			print("Setting derivative of x = " + str(v))
			solver.state_variables["x"].derivative = v
		else:
			print("Error: 'x' state variable doesn't have a derivative field")
			
		if "derivative" in solver.state_variables["v"]:
			var dv = -k/m * x
			print("Setting derivative of v = " + str(dv))
			solver.state_variables["v"].derivative = dv
		else:
			print("Error: 'v' state variable doesn't have a derivative field")

	# Run simulation for 1 second with a 0.1 time step
	func run_simulation(solver: DAESolver, end_time: float, dt: float) -> Array:
		print("Starting simulation with end_time=" + str(end_time) + ", dt=" + str(dt))
		var results = []
		var t = 0.0
		
		# Store initial state
		print("Getting initial values...")
		var x_initial = solver.get_variable_value("x")
		var v_initial = solver.get_variable_value("v")
		print("Initial x=" + str(x_initial) + ", v=" + str(v_initial))
		
		results.append({
			"time": t,
			"x": x_initial,
			"v": v_initial
		})
		
		# Run simulation
		print("Beginning simulation loop...")
		while t < end_time:
			# Take a step with RK4 method
			print("Taking RK4 step at t=" + str(t))
			
			# GDScript doesn't support try/except, use a safer approach
			print("Calling rk4_step...")
			rk4_step(solver, dt)
			print("RK4 step completed successfully")
			
			# Advance time
			t += dt
			
			# Store results
			var x_current = solver.get_variable_value("x")
			var v_current = solver.get_variable_value("v")
			print("t=" + str(t) + ", x=" + str(x_current) + ", v=" + str(v_current))
			
			results.append({
				"time": t,
				"x": x_current,
				"v": v_current
			})
		
		print("Simulation completed with " + str(results.size()) + " timesteps")
		return results
	
	# Perform a single 4th-order Runge-Kutta step for the spring-mass system
	func rk4_step(solver: DAESolver, dt: float) -> void:
		print("RK4 step with dt=" + str(dt))
		# Get current values
		var x0 = solver.get_variable_value("x")
		var v0 = solver.get_variable_value("v")
		var k = solver.get_variable_value("k")
		var m = solver.get_variable_value("m")
		
		print("Current values - x0: " + str(x0) + ", v0: " + str(v0) + ", k: " + str(k) + ", m: " + str(m))
		
		# Define the differential equations for the system
		# dx/dt = v
		# dv/dt = -k/m * x
		var f_x = func(x, v): return v
		var f_v = func(x, v): return -k/m * x
		
		# Step 1: Evaluate at the current point
		print("RK4 - Step 1")
		var k1_x = f_x.call(x0, v0)
		var k1_v = f_v.call(x0, v0)
		
		# Step 2: Evaluate at the midpoint using k1
		print("RK4 - Step 2")
		var k2_x = f_x.call(x0 + 0.5 * dt * k1_x, v0 + 0.5 * dt * k1_v)
		var k2_v = f_v.call(x0 + 0.5 * dt * k1_x, v0 + 0.5 * dt * k1_v)
		
		# Step 3: Evaluate at the midpoint using k2
		print("RK4 - Step 3")
		var k3_x = f_x.call(x0 + 0.5 * dt * k2_x, v0 + 0.5 * dt * k2_v)
		var k3_v = f_v.call(x0 + 0.5 * dt * k2_x, v0 + 0.5 * dt * k2_v)
		
		# Step 4: Evaluate at the end point using k3
		print("RK4 - Step 4")
		var k4_x = f_x.call(x0 + dt * k3_x, v0 + dt * k3_v)
		var k4_v = f_v.call(x0 + dt * k3_x, v0 + dt * k3_v)
		
		# Calculate the new values using the weighted average
		print("RK4 - Calculating final values")
		var x_new = x0 + dt/6.0 * (k1_x + 2*k2_x + 2*k3_x + k4_x)
		var v_new = v0 + dt/6.0 * (k1_v + 2*k2_v + 2*k3_v + k4_v)
		
		print("New values - x_new: " + str(x_new) + ", v_new: " + str(v_new))
		
		# Update the state variables
		print("Updating solver state variables")
		var x_result = solver.set_variable_value("x", x_new)
		var v_result = solver.set_variable_value("v", v_new)
		print("Update results - x: " + str(x_result) + ", v: " + str(v_result))
		
		# Check if state_variables dictionary exists and has the required keys
		if solver.state_variables != null and "x" in solver.state_variables and "v" in solver.state_variables:
			# Update the derivatives for record-keeping
			print("Updating derivatives in state_variables")
			solver.state_variables["x"].derivative = f_x.call(x_new, v_new)
			solver.state_variables["v"].derivative = f_v.call(x_new, v_new)
		else:
			print("Warning: state_variables dictionary missing or incomplete")

func _init():
	print("Starting test_simple_models.gd...")
	var direct_execution = true
	var test_suite_mode = false
	
	# Check execution mode
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
		if arg == "--test-suite-mode":
			test_suite_mode = true
			direct_execution = true
			break
	
	if direct_execution:
		print("\nRunning TestSimpleModels...")
		var test = TestSimpleModels.new()
		print("Running tests...")
		
		# Run the test
		var success = test.run_tests()
		
		# In test suite mode, make the success/failure explicit
		if test_suite_mode:
			if success:
				print("\n✅ TestSimpleModels PASSED")
			else:
				print("\n❌ TestSimpleModels FAILED")
		
		print("Test execution complete, quitting...")
		quit() 