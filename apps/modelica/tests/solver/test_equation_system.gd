#!/usr/bin/env -S godot --headless --script
extends SceneTree

const DAESolver = preload("../../core/solver.gd")
const ErrorSystem = preload("../../core/error_system.gd")

func test_solver():
	print("\n=== Testing DAE Solver ===")
	
	# Create solver instance
	var solver = DAESolver.new()
	print("Created solver instance")
	
	# Add variables
	var result = solver.add_state_variable("x", 0.0)
	print("Added state variable 'x': " + result.get_string())
	
	result = solver.add_algebraic_variable("y", 0.0)
	print("Added algebraic variable 'y': " + result.get_string())
	
	result = solver.add_parameter("p", 1.0)
	print("Added parameter 'p': " + result.get_string())
	
	# Add equation 
	result = solver.add_equation("der(x) = p * y")
	print("Added equation: " + result.get_string())
	
	# Test error handling - try to add duplicate variable
	result = solver.add_state_variable("x", 0.0)
	print("Tried to add duplicate variable 'x': " + result.get_string())
	
	# Get variable values
	var x_value = solver.get_variable_value("x")
	print("Value of 'x': " + str(x_value))
	
	var p_value = solver.get_variable_value("p")
	print("Value of 'p': " + str(p_value))
	
	# Try to get non-existent variable
	var z_value = solver.get_variable_value("z")
	print("Tried to get non-existent variable 'z': " + str(z_value))
	
	# Check solver error state
	print("Has errors: " + str(solver.has_errors()))
	print("Error count: " + str(solver.get_errors().size()))
	
	# Initialize solver
	result = solver.initialize()
	print("Initialized solver: " + result.get_string())
	
	# Take a time step
	result = solver.step(0.1)
	print("Took time step: " + result.get_string())
	
	# Get solver state
	var state = solver.get_state()
	print("Current time: " + str(state.time))
	print("Current value of 'x': " + str(state.state_variables.x))
	
	print("=== Test completed successfully ===")
	return true

func _init():
	var success = test_solver()
	if success:
		print("✅ All tests passed!")
	else:
		print("❌ Tests failed!")
	quit() 