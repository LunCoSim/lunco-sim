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
	
	func test_spring_mass_model():
		print("Setting up a simple Spring-Mass model")
		
		# Create an equation system
		var system = EquationSystem.new()
		
		# Add variables
		# m (parameter) - mass
		system.add_variable("m", {
			"is_parameter": true,
			"value": 1.0
		})
		
		# k (parameter) - spring constant
		system.add_variable("k", {
			"is_parameter": true,
			"value": 10.0
		})
		
		# x (state) - position
		system.add_variable("x", {
			"is_state": true,
			"value": 1.0  # Initial position x0
		})
		
		# v (state) - velocity
		system.add_variable("v", {
			"is_state": true,
			"value": 0.0  # Initial velocity v0
		})
		
		# F (algebraic) - force
		system.add_variable("F", {
			"value": 0.0
		})
		
		# Add equations
		# Equation 1: der(x) = v
		var eq1 = ModelicaEquation.new(
			ModelicaEquation.EquationType.DIFFERENTIAL,
			ModelicaExpression.create_derivative("x"),
			ModelicaExpression.create_variable("v")
		)
		system.add_equation(eq1)
		
		# Equation 2: der(v) = F/m
		var eq2 = ModelicaEquation.new(
			ModelicaEquation.EquationType.DIFFERENTIAL,
			ModelicaExpression.create_derivative("v"),
			ModelicaExpression.create_operator(
				"/",
				[
					ModelicaExpression.create_variable("F"),
					ModelicaExpression.create_variable("m")
				]
			)
		)
		system.add_equation(eq2)
		
		# Equation 3: F = -k*x
		var eq3 = ModelicaEquation.new(
			ModelicaEquation.EquationType.EXPLICIT,
			ModelicaExpression.create_variable("F"),
			ModelicaExpression.create_operator(
				"*",
				[
					ModelicaExpression.create_operator(
						"-",
						[ModelicaExpression.create_variable("k")]
					),
					ModelicaExpression.create_variable("x")
				]
			)
		)
		system.add_equation(eq3)
		
		print("Equation system created:")
		print(system)
		
		# Create RK4 solver directly
		print("Creating solver...")
		var solver = RK4Solver.new()
		solver.initialize(system)
		print("Solver created:")
		print(solver)
		
		# Simulate for 10 seconds with 0.01s time step
		var sim_time = 10.0
		var dt = 0.01
		var steps = int(sim_time / dt)
		
		# Analytical solution for comparison:
		# x(t) = x0 * cos(ω*t), where ω = sqrt(k/m)
		var omega = sqrt(system.get_variable_value("k") / system.get_variable_value("m"))
		var x0 = system.get_variable_value("x")
		
		print("Starting simulation...")
		print("Time,x,v,F,x_analytical,error")
		
		for i in range(steps + 1):
			var t = i * dt
			
			# Get current values
			var x = system.get_variable_value("x")
			var v = system.get_variable_value("v")
			var F = system.get_variable_value("F")
			
			# Calculate analytical solution for comparison
			var x_analytical = x0 * cos(omega * t)
			var error = abs(x - x_analytical)
			
			# Print current state
			if i % 100 == 0:  # Print every 100 steps to keep output manageable
				print("%f,%f,%f,%f,%f,%f" % [t, x, v, F, x_analytical, error])
			
			# Take a step
			if i < steps:
				solver.step(dt)
		
		print("Simulation complete")
		
		# Check final error
		var final_t = steps * dt
		var final_x = system.get_variable_value("x")
		var final_x_analytical = x0 * cos(omega * final_t)
		var final_error = abs(final_x - final_x_analytical)
		
		print("Final time: %f" % final_t)
		print("Final position: %f" % final_x)
		print("Final analytical position: %f" % final_x_analytical)
		print("Final error: %f" % final_error)
		
		# Test assertion
		assert(final_error < 0.01, "Final error is too high: %f" % final_error)
		
		print("Test passed!")

func _init():
	var direct_execution = true
	
	# Check if we're being run directly or via the test runner
	for arg in OS.get_cmdline_args():
		if arg.ends_with("run_tests.gd"):
			direct_execution = false
			break
	
	if direct_execution:
		print("\nRunning TestNewArchitecture...")
		var test = TestNewArchitecture.new()
		test.run_tests()
		quit()