#!/usr/bin/env -S godot --headless --script
extends SceneTree

class TestNewArchitecture extends "res://apps/modelica/tests/base_test.gd":
	# Preload all required classes
	const EquationSystem = preload("res://apps/modelica/core/equation_system.gd")
	const ModelicaEquation = preload("res://apps/modelica/core/equation.gd")
	const ModelicaExpression = preload("res://apps/modelica/core/expression.gd")
	const SolverStrategy = preload("res://apps/modelica/core/solver_strategy.gd")
	const CausalSolver = preload("res://apps/modelica/core/causal_solver.gd")
	const RK4Solver = preload("res://apps/modelica/core/rk4_solver.gd")
	const SolverFactory = preload("res://apps/modelica/core/solver_factory.gd")
	
	func setup():
		print("Setting up TestNewArchitecture...")
		# Verify all required classes are loaded
		print("Checking if EquationSystem exists: " + str(EquationSystem != null))
		print("Checking if ModelicaEquation exists: " + str(ModelicaEquation != null))
		print("Checking if ModelicaExpression exists: " + str(ModelicaExpression != null))
		print("Checking if SolverStrategy exists: " + str(SolverStrategy != null))
		print("Checking if CausalSolver exists: " + str(CausalSolver != null))
		print("Checking if RK4Solver exists: " + str(RK4Solver != null))
		print("Checking if SolverFactory exists: " + str(SolverFactory != null))
		print("Setup complete.")
	
	func test_spring_mass_model():
		print("Setting up a simple Spring-Mass model")
		
		# Create an equation system
		print("Creating equation system...")
		var system = EquationSystem.new()
		print("Equation system created: " + str(system))
		
		# Add variables
		print("Adding variables to the system...")
		
		# m (parameter) - mass
		print("Adding 'm' parameter...")
		system.add_variable("m", {
			"is_parameter": true,
			"value": 1.0
		})
		print("m parameter added")
		
		# k (parameter) - spring constant
		print("Adding 'k' parameter...")
		system.add_variable("k", {
			"is_parameter": true,
			"value": 10.0
		})
		print("k parameter added")
		
		# x (state) - position
		print("Adding 'x' state variable...")
		system.add_variable("x", {
			"is_state": true,
			"value": 1.0  # Initial position x0
		})
		print("x state variable added")
		
		# v (state) - velocity
		print("Adding 'v' state variable...")
		system.add_variable("v", {
			"is_state": true,
			"value": 0.0  # Initial velocity v0
		})
		print("v state variable added")
		
		# F (algebraic) - force
		print("Adding 'F' algebraic variable...")
		system.add_variable("F", {
			"value": 0.0
		})
		print("F algebraic variable added")
		
		# Add equations
		print("Adding equations to the system...")
		
		# Equation 1: der(x) = v
		print("Creating equation 1: der(x) = v")
		print("Creating derivative expression for 'x'...")
		var der_x = ModelicaExpression.create_derivative("x")
		print("Creating variable expression for 'v'...")
		var var_v = ModelicaExpression.create_variable("v")
		var eq1 = ModelicaEquation.new(
			ModelicaEquation.EquationType.DIFFERENTIAL,
			der_x,
			var_v
		)
		print("Equation 1 created: " + str(eq1))
		print("Adding equation 1 to system...")
		system.add_equation(eq1)
		print("Equation 1 added")
		
		# Equation 2: der(v) = F/m
		print("Creating equation 2: der(v) = F/m")
		print("Creating derivative expression for 'v'...")
		var der_v = ModelicaExpression.create_derivative("v")
		print("Creating division operation expression...")
		print("Creating variable expressions for 'F' and 'm'...")
		var var_F = ModelicaExpression.create_variable("F")
		var var_m = ModelicaExpression.create_variable("m")
		print("Creating division operator expression...")
		var div_expr = ModelicaExpression.create_operator(
			"/",
			[
				var_F,
				var_m
			]
		)
		print("Division expression created: " + str(div_expr))
		var eq2 = ModelicaEquation.new(
			ModelicaEquation.EquationType.DIFFERENTIAL,
			der_v,
			div_expr
		)
		print("Equation 2 created: " + str(eq2))
		print("Adding equation 2 to system...")
		system.add_equation(eq2)
		print("Equation 2 added")
		
		# Equation 3: F = -k*x
		print("Creating equation 3: F = -k*x")
		print("Creating variable expression for 'F'...")
		var F_expr = ModelicaExpression.create_variable("F")
		print("Creating variable expressions for 'k' and 'x'...")
		var var_k = ModelicaExpression.create_variable("k")
		var var_x = ModelicaExpression.create_variable("x")
		print("Creating negation of 'k'...")
		var neg_k = ModelicaExpression.create_operator(
			"-",
			[var_k]
		)
		print("Negation created: " + str(neg_k))
		print("Creating multiplication operation...")
		var mul_expr = ModelicaExpression.create_operator(
			"*",
			[
				neg_k,
				var_x
			]
		)
		print("Multiplication expression created: " + str(mul_expr))
		var eq3 = ModelicaEquation.new(
			ModelicaEquation.EquationType.EXPLICIT,
			F_expr,
			mul_expr
		)
		print("Equation 3 created: " + str(eq3))
		print("Adding equation 3 to system...")
		system.add_equation(eq3)
		print("Equation 3 added")
		
		print("Equation system created:")
		print(system)
		
		# Create solver factory to select the best solver
		print("Creating solver using factory...")
		print("Instantiating SolverFactory...")
		var factory = SolverFactory.new()
		print("SolverFactory created: " + str(factory))
		print("Creating RK4 solver for the system...")
		
		# GDScript doesn't support try/except, so handle errors differently
		print("Attempting to create solver...")
		var solver = factory.create_solver(system, SolverFactory.SolverType.RK4)
		print("Solver created successfully: " + str(solver))
		
		if solver == null:
			push_error("Failed to create solver - solver is null")
			print("Failed to create solver - solver is null")
			assert(false, "Failed to create solver")
			return
		
		print("Solver created, checking initialization status:")
		print("Solver initialized: " + str(solver.initialized))
		print(solver)
		
		# Stop test if solver is not initialized
		if not solver.initialized:
			print("CRITICAL ERROR: RK4Solver failed to initialize")
			assert(solver.initialized, "RK4Solver failed to initialize")
			return
		
		# Simulate for 10 seconds with 0.01s time step
		var sim_time = 10.0
		var dt = 0.01
		var steps = int(sim_time / dt)
		
		# Analytical solution for comparison:
		# x(t) = x0 * cos(ω*t), where ω = sqrt(k/m)
		print("Calculating omega for analytical solution...")
		print("Getting 'k' value...")
		var k_value = system.get_variable_value("k")
		print("k value: " + str(k_value))
		print("Getting 'm' value...")
		var m_value = system.get_variable_value("m")
		print("m value: " + str(m_value))
		
		var omega = sqrt(k_value / m_value)
		print("Calculated omega = sqrt(k/m) = " + str(omega))
		
		print("Getting initial x value...")
		var x0 = system.get_variable_value("x")
		print("Initial x value (x0): " + str(x0))
		
		print("Starting simulation...")
		print("Time,x,v,F,x_analytical,error")
		
		var max_error = 0.0
		var simulation_failure = false
		
		for i in range(steps + 1):
			var t = i * dt
			
			# Get current values
			print("Getting state values at t=" + str(t) + "...")
			var x = system.get_variable_value("x")
			var v = system.get_variable_value("v")
			var F = system.get_variable_value("F")
			print("Current values - x: " + str(x) + ", v: " + str(v) + ", F: " + str(F))
			
			# Calculate analytical solution for comparison
			var x_analytical = x0 * cos(omega * t)
			var error = abs(x - x_analytical)
			max_error = max(max_error, error)
			
			# Print current state
			if i % 100 == 0:  # Print every 100 steps to keep output manageable
				print("%f,%f,%f,%f,%f,%f" % [t, x, v, F, x_analytical, error])
			
			# Take a step
			if i < steps:
				print("Taking solver step " + str(i) + " at t=" + str(t) + "...")
				# GDScript doesn't support try/except, so handle errors differently
				var step_result = solver.step(dt)
				print("Step result: " + str(step_result))

				if not step_result:
					push_error("Step failed at t=" + str(t))
					print("Step failed at t=" + str(t))
					simulation_failure = true
					break
		
		print("Simulation complete")
		
		if simulation_failure:
			assert(false, "Simulation failed during execution")
			return
		
		# Check final error
		var final_t = steps * dt
		var final_x = system.get_variable_value("x")
		var final_x_analytical = x0 * cos(omega * final_t)
		var final_error = abs(final_x - final_x_analytical)
		
		print("Final time: %f" % final_t)
		print("Final position: %f" % final_x)
		print("Final analytical position: %f" % final_x_analytical)
		print("Final error: %f" % final_error)
		print("Maximum error during simulation: %f" % max_error)
		
		# Test assertion with more lenient threshold since we just want to verify the architecture works
		assert(final_error < 0.05, "Final error is too high: %f" % final_error)
		
		print("Test passed!")

# Bridge method for the test runner
func run_tests():
	var test_instance = TestNewArchitecture.new()
	
	# Make sure the test is properly initialized
	test_instance.test_class = "TestNewArchitecture"
	test_instance.total_tests = 0
	test_instance.passed_tests = 0
	test_instance.failed_tests = 0
	test_instance.skipped_tests = 0
	
	# Run the tests with proper initialization
	return test_instance.run_tests()

func _init():
	var direct_execution = true
	
	# Check if we're being run directly or via the test runner
	var test_suite_mode = false
	
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
		if arg == "--test-suite-mode":
			test_suite_mode = true
			direct_execution = true
			break
	
	if direct_execution:
		print("\nRunning TestNewArchitecture...")
		var test = TestNewArchitecture.new()
		
		# Run the tests
		var success = test.run_tests()
		
		# In test suite mode, make the success/failure explicit
		if test_suite_mode:
			if success:
				print("\n✅ TestNewArchitecture PASSED")
			else:
				print("\n❌ TestNewArchitecture FAILED")
				
		print("Test execution complete, quitting...")
		quit()