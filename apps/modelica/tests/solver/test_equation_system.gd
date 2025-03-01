extends "../base_test.gd"

const DAESolver = preload("../../core/solver.gd")

var solver: DAESolver

func setup():
	solver = DAESolver.new()

func test_add_state_variables():
	# Test adding state variables
	solver.add_state_variable("x", 1.0)
	solver.add_state_variable("v", 0.0)
	
	# Check if variables were added
	assert_equal(solver.state_variables.size(), 2, "Should have 2 state variables")
	assert_has(solver.state_variables, "x", "Should have variable 'x'")
	assert_has(solver.state_variables, "v", "Should have variable 'v'")
	
	# Check values
	assert_equal(solver.state_variables["x"].value, 1.0, "x should be initialized to 1.0")
	assert_equal(solver.state_variables["v"].value, 0.0, "v should be initialized to 0.0")
	
	# Check derivatives (should be zero initially)
	assert_equal(solver.state_variables["x"].derivative, 0.0, "x derivative should be zero")
	assert_equal(solver.state_variables["v"].derivative, 0.0, "v derivative should be zero")

func test_add_algebraic_variables():
	# Test adding algebraic variables
	solver.add_algebraic_variable("y", 2.0)
	solver.add_algebraic_variable("z", 3.0)
	
	# Check if variables were added
	assert_equal(solver.algebraic_variables.size(), 2, "Should have 2 algebraic variables")
	assert_has(solver.algebraic_variables, "y", "Should have variable 'y'")
	assert_has(solver.algebraic_variables, "z", "Should have variable 'z'")
	
	# Check values
	assert_equal(solver.algebraic_variables["y"].value, 2.0, "y should be initialized to 2.0")
	assert_equal(solver.algebraic_variables["z"].value, 3.0, "z should be initialized to 3.0")

func test_add_parameters():
	# Test adding parameters
	solver.add_parameter("m", 1.0)
	solver.add_parameter("k", 10.0)
	solver.add_parameter("c", 0.5)
	
	# Check if parameters were added
	assert_equal(solver.parameters.size(), 3, "Should have 3 parameters")
	assert_has(solver.parameters, "m", "Should have parameter 'm'")
	assert_has(solver.parameters, "k", "Should have parameter 'k'")
	assert_has(solver.parameters, "c", "Should have parameter 'c'")
	
	# Check values
	assert_equal(solver.parameters["m"], 1.0, "m should be initialized to 1.0")
	assert_equal(solver.parameters["k"], 10.0, "k should be initialized to 10.0")
	assert_equal(solver.parameters["c"], 0.5, "c should be initialized to 0.5")

func test_add_equation():
	# Test adding equations
	solver.add_equation("v = der(x)")
	solver.add_equation("m*der(v) + c*v + k*x = 0")
	
	# Check if equations were added
	assert_equal(solver.equations.size(), 2, "Should have 2 equations")
	assert_equal(solver.equations[0], "v = der(x)", "First equation should be correct")
	assert_equal(solver.equations[1], "m*der(v) + c*v + k*x = 0", "Second equation should be correct")

func test_get_variable_value():
	# Add variables and parameters
	solver.add_state_variable("x", 1.0)
	solver.add_algebraic_variable("y", 2.0)
	solver.add_parameter("p", 3.0)
	
	# Test getting values
	assert_equal(solver.get_variable_value("x"), 1.0, "Should get correct value for x")
	assert_equal(solver.get_variable_value("y"), 2.0, "Should get correct value for y")
	assert_equal(solver.get_variable_value("p"), 3.0, "Should get correct value for p")
	
	# Test getting non-existent variable
	# This should trigger an error, but we'll capture it with assert_throws
	var callable = func(): return solver.get_variable_value("z")
	assert_throws(callable, "Should throw error for non-existent variable")

func test_set_variable_value():
	# Add variables
	solver.add_state_variable("x", 1.0)
	solver.add_algebraic_variable("y", 2.0)
	
	# Set new values
	solver.set_variable_value("x", 5.0)
	solver.set_variable_value("y", 6.0)
	
	# Check if values were updated
	assert_equal(solver.get_variable_value("x"), 5.0, "x should be updated to 5.0")
	assert_equal(solver.get_variable_value("y"), 6.0, "y should be updated to 6.0")
	
	# Test setting non-existent variable
	var callable = func(): solver.set_variable_value("z", 7.0)
	assert_throws(callable, "Should throw error for non-existent variable")

func test_simple_step():
	# Set up a simple system: x' = v, v' = -x
	solver.add_state_variable("x", 1.0)
	solver.add_state_variable("v", 0.0)
	
	# Set derivatives directly (normally this would be done by the equation solver)
	solver.state_variables["x"].derivative = solver.state_variables["v"].value
	solver.state_variables["v"].derivative = -solver.state_variables["x"].value
	
	# Take a step with dt=0.1
	var success = solver.step(0.1)
	assert_true(success, "Step should succeed")
	
	# Check updated values (using Euler method)
	# x = 1.0 + 0.0 * 0.1 = 1.0
	# v = 0.0 + (-1.0) * 0.1 = -0.1
	assert_almost_equal(solver.get_variable_value("x"), 1.0, 1e-6, "x should be updated correctly")
	assert_almost_equal(solver.get_variable_value("v"), -0.1, 1e-6, "v should be updated correctly")
	
	# Take another step
	solver.state_variables["x"].derivative = solver.state_variables["v"].value
	solver.state_variables["v"].derivative = -solver.state_variables["x"].value
	
	success = solver.step(0.1)
	assert_true(success, "Second step should succeed")
	
	# Check updated values
	# x = 1.0 + (-0.1) * 0.1 = 0.99
	# v = -0.1 + (-1.0) * 0.1 = -0.2
	assert_almost_equal(solver.get_variable_value("x"), 0.99, 1e-6, "x should be updated correctly after second step")
	assert_almost_equal(solver.get_variable_value("v"), -0.2, 1e-6, "v should be updated correctly after second step")

func test_get_state():
	# Set up a system
	solver.add_state_variable("x", 1.0)
	solver.add_state_variable("v", 0.0)
	solver.add_algebraic_variable("a", -1.0)
	solver.add_parameter("m", 1.0)
	
	# Get state
	var state = solver.get_state()
	
	# Check state contents
	assert_equal(state.time, 0.0, "Initial time should be 0.0")
	assert_equal(state.state_variables.size(), 2, "Should have 2 state variables in state")
	assert_equal(state.algebraic_variables.size(), 1, "Should have 1 algebraic variable in state")
	assert_equal(state.parameters.size(), 1, "Should have 1 parameter in state")
	
	# Check specific values
	assert_equal(state.state_variables["x"], 1.0, "x should be 1.0 in state")
	assert_equal(state.state_variables["v"], 0.0, "v should be 0.0 in state")
	assert_equal(state.algebraic_variables["a"], -1.0, "a should be -1.0 in state")
	assert_equal(state.parameters["m"], 1.0, "m should be 1.0 in state") 