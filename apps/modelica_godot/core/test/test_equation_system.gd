extends SceneTree

const EquationSystem = preload("../equation_system.gd")
const ModelicaComponent = preload("../component.gd")
const ModelicaConnector = preload("../connector.gd")

var tests_run := 0
var tests_passed := 0
var current_test := ""

func _init():
	print("\nRunning Equation System Tests...")
	_run_all_tests()
	print("\nTests completed: %d/%d passed" % [tests_passed, tests_run])
	quit()

func _run_all_tests() -> void:
	_test_basic_equations()
	_test_spring_system()
	_test_mass_system()
	_test_spring_mass_system()
	_test_initial_conditions()
	_test_derivatives()
	_test_damped_spring_mass_system()
	_test_coupled_pendulums()
	_test_bouncing_ball()
	_test_ast_dependencies()
	_test_ast_differential()
	_test_ast_state_variables()

func _start_test(test_name: String) -> void:
	current_test = test_name
	tests_run += 1
	print("\nRunning test: " + test_name)

func _assert(condition: bool, message: String) -> void:
	if condition:
		tests_passed += 1
		print("  ✓ " + message)
	else:
		print("  ✗ " + message)
		push_error("Test failed: " + current_test + " - " + message)

func _assert_approx(a: float, b: float, message: String, tolerance: float = 1e-6) -> void:
	_assert(abs(a - b) < tolerance, message + " (got %.6f, expected %.6f)" % [a, b])

func _test_basic_equations() -> void:
	_start_test("Basic Equations")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add variables to component
	comp.add_variable("x")
	comp.add_variable("y")
	comp.add_variable("z")
	comp.add_variable("w")
	comp.add_variable("v")
	
	# Test simple equation
	eq_system.add_component(comp)
	eq_system.add_equation("test.x = 5.0", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("x"), 5.0, "Simple constant assignment")
	
	# Test basic arithmetic
	eq_system = EquationSystem.new()
	eq_system.add_component(comp)
	eq_system.add_equation("test.y = 3.0 + 2.0", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("y"), 5.0, "Addition")
	
	eq_system = EquationSystem.new()
	eq_system.add_component(comp)
	eq_system.add_equation("test.z = 6.0 - 2.0", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("z"), 4.0, "Subtraction")
	
	eq_system = EquationSystem.new()
	eq_system.add_component(comp)
	eq_system.add_equation("test.w = 3.0 * 2.0", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("w"), 6.0, "Multiplication")
	
	eq_system = EquationSystem.new()
	eq_system.add_component(comp)
	eq_system.add_equation("test.v = 6.0 / 2.0", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("v"), 3.0, "Division")

func _test_spring_system() -> void:
	_start_test("Spring System")
	
	var eq_system = EquationSystem.new()
	var spring = ModelicaComponent.new("spring")
	
	# Add parameters and variables to spring component
	spring.add_parameter("k", 100.0)  # N/m
	spring.add_parameter("l0", 1.0)   # m
	spring.add_variable("length")
	spring.add_variable("elongation")
	spring.add_variable("force")
	
	# Add spring equations
	eq_system.add_component(spring)
	eq_system.add_equation("spring.length = 1.5", spring)  # Fixed length for testing
	eq_system.add_equation("spring.elongation = spring.length - spring.l0", spring)
	eq_system.add_equation("spring.force = spring.k * spring.elongation", spring)
	
	eq_system.initialize()
	
	# Test spring calculations
	_assert_approx(spring.get_variable("length"), 1.5, "Spring length")
	_assert_approx(spring.get_variable("elongation"), 0.5, "Spring elongation")
	_assert_approx(spring.get_variable("force"), 50.0, "Spring force")

func _test_mass_system() -> void:
	_start_test("Mass System")
	
	var eq_system = EquationSystem.new()
	var mass = ModelicaComponent.new("mass")
	
	# Add parameters and variables to mass component
	mass.add_parameter("m", 1.0)  # kg
	mass.add_state_variable("position")
	mass.add_state_variable("velocity")
	mass.add_variable("force")
	mass.add_variable("acceleration")
	
	# Add mass equations with constant force
	eq_system.add_component(mass)
	eq_system.add_initial_condition("mass.position", 0.0, mass)
	eq_system.add_initial_condition("mass.velocity", 0.0, mass)
	eq_system.add_equation("mass.force = 10.0", mass)  # Constant force
	eq_system.add_equation("mass.acceleration = mass.force / mass.m", mass)
	eq_system.add_equation("der(mass.position) = mass.velocity", mass)
	eq_system.add_equation("der(mass.velocity) = mass.acceleration", mass)
	
	eq_system.initialize()
	eq_system.solve_step()  # Single time step
	
	# Test mass motion under constant force
	# Using dt = 0.01:
	# a = F/m = 10 m/s²
	# v = v0 + a*dt = 0 + 10*0.01 = 0.1 m/s
	# x = x0 + v0*dt = 0 + 0*0.01 = 0.0 m  # Initial velocity was 0
	_assert_approx(mass.get_variable("acceleration"), 10.0, "Mass acceleration")
	_assert_approx(mass.get_variable("velocity"), 0.1, "Mass velocity after one step")
	_assert_approx(mass.get_variable("position"), 0.0, "Mass position after one step")

func _test_spring_mass_system() -> void:
	_start_test("Spring-Mass System")
	
	var eq_system = EquationSystem.new()
	
	# Create spring component
	var spring = ModelicaComponent.new("spring")
	spring.add_parameter("k", 100.0)  # N/m
	spring.add_parameter("l0", 1.0)   # m
	spring.add_variable("length")
	spring.add_variable("elongation")
	spring.add_variable("force")
	
	# Create mass component
	var mass = ModelicaComponent.new("mass")
	mass.add_parameter("m", 1.0)  # kg
	mass.add_state_variable("position")
	mass.add_state_variable("velocity")
	mass.add_variable("force")
	mass.add_variable("acceleration")
	
	# Add components
	eq_system.add_component(spring)
	eq_system.add_component(mass)
	
	# Add initial conditions
	eq_system.add_initial_condition("mass.position", 1.5, mass)  # Stretched 0.5m
	eq_system.add_initial_condition("mass.velocity", 0.0, mass)
	
	# Add equations
	eq_system.add_equation("spring.length = mass.position", spring)
	eq_system.add_equation("spring.elongation = spring.length - spring.l0", spring)
	eq_system.add_equation("spring.force = spring.k * spring.elongation", spring)
	eq_system.add_equation("mass.force = -spring.force", mass)  # Newton's 3rd law
	eq_system.add_equation("mass.acceleration = mass.force / mass.m", mass)
	eq_system.add_equation("der(mass.position) = mass.velocity", mass)
	eq_system.add_equation("der(mass.velocity) = mass.acceleration", mass)
	
	eq_system.initialize()
	
	# Test initial state
	_assert_approx(spring.get_variable("elongation"), 0.5, "Initial spring elongation")
	_assert_approx(spring.get_variable("force"), 50.0, "Initial spring force")
	_assert_approx(mass.get_variable("acceleration"), -50.0, "Initial mass acceleration")
	
	# Test first time step
	eq_system.solve_step()
	# After one step (dt = 0.01):
	# a = -50 m/s²
	# v = v0 + a*dt = 0 + (-50)*0.01 = -0.5 m/s
	# x = x0 + v0*dt = 1.5 + 0*0.01 = 1.5 m  # Initial velocity was 0
	_assert_approx(mass.get_variable("velocity"), -0.5, "Mass moving towards equilibrium")
	_assert_approx(mass.get_variable("position"), 1.5, "Mass position unchanged in first step")

func _test_initial_conditions() -> void:
	_start_test("Initial Conditions")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add variables
	comp.add_variable("x")
	comp.add_variable("y")
	comp.add_variable("v")
	
	# Test multiple initial conditions
	eq_system.add_component(comp)
	eq_system.add_initial_condition("test.x", 1.0, comp)
	eq_system.add_initial_condition("test.v", 2.0, comp)
	eq_system.initialize()
	
	_assert_approx(comp.get_variable("x"), 1.0, "First initial condition")
	_assert_approx(comp.get_variable("v"), 2.0, "Second initial condition")
	
	# Test initial condition with dependent equation
	eq_system.add_equation("test.y = 2.0 * test.x", comp)
	eq_system.initialize()
	_assert_approx(comp.get_variable("y"), 2.0, "Dependent variable initialization")

func _test_derivatives() -> void:
	_start_test("Derivatives")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add state variables
	comp.add_state_variable("x")
	comp.add_state_variable("pos")
	comp.add_state_variable("vel")
	
	# Test simple derivative
	eq_system.add_component(comp)
	eq_system.add_initial_condition("test.x", 0.0, comp)
	eq_system.add_equation("der(test.x) = 1.0", comp)
	
	eq_system.initialize()
	eq_system.solve_step()
	
	# After one step (dt = 0.01), x should be 0.01
	_assert_approx(comp.get_variable("x"), 0.01, "Simple derivative integration")
	
	# Test coupled derivatives
	eq_system.clear()
	eq_system.add_component(comp)
	eq_system.add_initial_condition("test.pos", 0.0, comp)
	eq_system.add_initial_condition("test.vel", 1.0, comp)
	eq_system.add_equation("der(test.pos) = test.vel", comp)
	eq_system.add_equation("der(test.vel) = -test.pos", comp)  # Simple harmonic motion
	
	eq_system.initialize()
	eq_system.solve_step()
	
	# After one step (dt = 0.01):
	# a = -x = 0.0 initially
	# v = v0 + a*dt = 1.0 + 0.0*0.01 = 1.0
	# x = x0 + v0*dt = 0.0 + 1.0*0.01 = 0.01
	_assert_approx(comp.get_variable("pos"), 0.01, "Position increasing with positive velocity")
	_assert_approx(comp.get_variable("vel"), 1.0, "Velocity unchanged in first step")

func _test_damped_spring_mass_system() -> void:
	_start_test("Damped Spring-Mass System")
	
	var eq_system = EquationSystem.new()
	
	# Create spring component
	var spring = ModelicaComponent.new("spring")
	spring.add_parameter("k", 100.0)  # N/m
	spring.add_parameter("l0", 1.0)   # m
	spring.add_variable("length")
	spring.add_variable("elongation")
	spring.add_variable("force")
	
	# Create damper component
	var damper = ModelicaComponent.new("damper")
	damper.add_parameter("c", 10.0)  # Ns/m - damping coefficient
	damper.add_variable("velocity")
	damper.add_variable("force")
	
	# Create mass component
	var mass = ModelicaComponent.new("mass")
	mass.add_parameter("m", 1.0)  # kg
	mass.add_state_variable("position")
	mass.add_state_variable("velocity")
	mass.add_variable("force")
	mass.add_variable("acceleration")
	
	# Add components
	eq_system.add_component(spring)
	eq_system.add_component(damper)
	eq_system.add_component(mass)
	
	# Add initial conditions
	eq_system.add_initial_condition("mass.position", 1.5, mass)  # Stretched 0.5m
	eq_system.add_initial_condition("mass.velocity", 0.0, mass)
	
	# Add equations
	eq_system.add_equation("spring.length = mass.position", spring)
	eq_system.add_equation("spring.elongation = spring.length - spring.l0", spring)
	eq_system.add_equation("spring.force = spring.k * spring.elongation", spring)
	eq_system.add_equation("damper.velocity = mass.velocity", damper)
	eq_system.add_equation("damper.force = damper.c * damper.velocity", damper)
	eq_system.add_equation("mass.force = -(spring.force + damper.force)", mass)  # Total force on mass
	eq_system.add_equation("mass.acceleration = mass.force / mass.m", mass)
	eq_system.add_equation("der(mass.position) = mass.velocity", mass)
	eq_system.add_equation("der(mass.velocity) = mass.acceleration", mass)
	
	eq_system.initialize()
	
	# Test initial state
	_assert_approx(spring.get_variable("elongation"), 0.5, "Initial spring elongation")
	_assert_approx(spring.get_variable("force"), 50.0, "Initial spring force")
	_assert_approx(damper.get_variable("force"), 0.0, "Initial damper force")
	_assert_approx(mass.get_variable("acceleration"), -50.0, "Initial mass acceleration")
	
	# Test first time step
	eq_system.solve_step()
	# After one step (dt = 0.01):
	# a = -50 m/s²
	# v = v0 + a*dt = 0 + (-50)*0.01 = -0.5 m/s
	# Damper force = c*v = 10 * (-0.5) = -5 N
	# x = x0 + v0*dt = 1.5 + 0*0.01 = 1.5 m  # Initial velocity was 0
	_assert_approx(mass.get_variable("velocity"), -0.5, "Mass velocity after first step")
	_assert_approx(mass.get_variable("position"), 1.5, "Mass position after first step")
	_assert_approx(damper.get_variable("force"), -5.0, "Damper force after first step")

func _test_coupled_pendulums() -> void:
	_start_test("Coupled Pendulums")
	
	var eq_system = EquationSystem.new()
	
	# Create two pendulums connected by a spring
	var pendulum1 = ModelicaComponent.new("pendulum1")
	pendulum1.add_parameter("m", 1.0)  # kg
	pendulum1.add_parameter("L", 1.0)  # m - length
	pendulum1.add_parameter("g", 9.81)  # m/s² - gravity
	pendulum1.add_state_variable("theta")  # angle from vertical
	pendulum1.add_state_variable("omega")  # angular velocity
	pendulum1.add_variable("x")  # horizontal position
	pendulum1.add_variable("y")  # vertical position
	pendulum1.add_variable("torque")
	
	var pendulum2 = ModelicaComponent.new("pendulum2")
	pendulum2.add_parameter("m", 1.0)
	pendulum2.add_parameter("L", 1.0)
	pendulum2.add_parameter("g", 9.81)
	pendulum2.add_state_variable("theta")
	pendulum2.add_state_variable("omega")
	pendulum2.add_variable("x")
	pendulum2.add_variable("y")
	pendulum2.add_variable("torque")
	
	var spring = ModelicaComponent.new("spring")
	spring.add_parameter("k", 10.0)  # N/m
	spring.add_variable("length")
	spring.add_variable("force")
	
	# Add components
	eq_system.add_component(pendulum1)
	eq_system.add_component(pendulum2)
	eq_system.add_component(spring)
	
	# Add initial conditions
	eq_system.add_initial_condition("pendulum1.theta", 0.1, pendulum1)  # Small initial angle
	eq_system.add_initial_condition("pendulum1.omega", 0.0, pendulum1)
	eq_system.add_initial_condition("pendulum2.theta", -0.1, pendulum2)  # Opposite small angle
	eq_system.add_initial_condition("pendulum2.omega", 0.0, pendulum2)
	
	# Add equations for pendulum positions
	eq_system.add_equation("pendulum1.x = pendulum1.L * sin(pendulum1.theta)", pendulum1)
	eq_system.add_equation("pendulum1.y = -pendulum1.L * cos(pendulum1.theta)", pendulum1)
	eq_system.add_equation("pendulum2.x = pendulum2.L * sin(pendulum2.theta)", pendulum2)
	eq_system.add_equation("pendulum2.y = -pendulum2.L * cos(pendulum2.theta)", pendulum2)
	
	# Spring force
	eq_system.add_equation("spring.length = sqrt((pendulum2.x - pendulum1.x)^2 + (pendulum2.y - pendulum1.y)^2)", spring)
	eq_system.add_equation("spring.force = spring.k * spring.length", spring)
	
	# Pendulum dynamics
	eq_system.add_equation("pendulum1.torque = -pendulum1.m * pendulum1.g * pendulum1.L * sin(pendulum1.theta)", pendulum1)
	eq_system.add_equation("pendulum2.torque = -pendulum2.m * pendulum2.g * pendulum2.L * sin(pendulum2.theta)", pendulum2)
	
	# Angular acceleration equations
	eq_system.add_equation("der(pendulum1.theta) = pendulum1.omega", pendulum1)
	eq_system.add_equation("der(pendulum1.omega) = pendulum1.torque / (pendulum1.m * pendulum1.L^2)", pendulum1)
	eq_system.add_equation("der(pendulum2.theta) = pendulum2.omega", pendulum2)
	eq_system.add_equation("der(pendulum2.omega) = pendulum2.torque / (pendulum2.m * pendulum2.L^2)", pendulum2)
	
	eq_system.initialize()
	
	# Test initial state
	_assert_approx(pendulum1.get_variable("theta"), 0.1, "Initial angle of pendulum 1")
	_assert_approx(pendulum2.get_variable("theta"), -0.1, "Initial angle of pendulum 2")
	_assert_approx(pendulum1.get_variable("omega"), 0.0, "Initial angular velocity of pendulum 1")
	_assert_approx(pendulum2.get_variable("omega"), 0.0, "Initial angular velocity of pendulum 2")
	
	# Test first time step
	eq_system.solve_step()
	
	# After one step (dt = 0.01):
	# For small angles, sin(theta) ≈ theta
	# torque1 ≈ -m*g*L*theta = -9.81*0.1 = -0.981 N⋅m
	# alpha1 = torque1/(m*L^2) = -0.981 rad/s²
	# omega1 = omega0 + alpha1*dt = 0 + (-0.981)*0.01 = -0.00981 rad/s
	_assert_approx(pendulum1.get_variable("omega"), -0.00981, "Angular velocity of pendulum 1 after first step", 1e-5)
	_assert_approx(pendulum2.get_variable("omega"), 0.00981, "Angular velocity of pendulum 2 after first step", 1e-5)

func _test_bouncing_ball() -> void:
	_start_test("Bouncing Ball")
	
	var eq_system = EquationSystem.new()
	
	# Create ball component
	var ball = ModelicaComponent.new("ball")
	ball.add_parameter("m", 1.0)  # kg
	ball.add_parameter("g", 9.81)  # m/s²
	ball.add_parameter("e", 0.8)   # coefficient of restitution
	ball.add_state_variable("height")  # y position
	ball.add_state_variable("velocity")  # vertical velocity
	ball.add_variable("force")
	ball.add_variable("acceleration")
	
	# Add component
	eq_system.add_component(ball)
	
	# Add initial conditions
	eq_system.add_initial_condition("ball.height", 1.0, ball)  # Start at 1m height
	eq_system.add_initial_condition("ball.velocity", 0.0, ball)  # Start at rest
	
	# Add equations
	eq_system.add_equation("ball.force = -ball.m * ball.g", ball)  # Gravity force
	eq_system.add_equation("ball.acceleration = ball.force / ball.m", ball)
	eq_system.add_equation("der(ball.height) = ball.velocity", ball)
	eq_system.add_equation("der(ball.velocity) = ball.acceleration", ball)
	
	eq_system.initialize()
	
	# Test initial state
	_assert_approx(ball.get_variable("height"), 1.0, "Initial height")
	_assert_approx(ball.get_variable("velocity"), 0.0, "Initial velocity")
	_assert_approx(ball.get_variable("acceleration"), -9.81, "Initial acceleration")
	
	# Test first time step
	eq_system.solve_step()
	
	# After one step (dt = 0.01):
	# a = -g = -9.81 m/s²
	# v = v0 + a*dt = 0 + (-9.81)*0.01 = -0.0981 m/s
	# h = h0 + v0*dt = 1.0 + 0*0.01 = 1.0 m
	_assert_approx(ball.get_variable("velocity"), -0.0981, "Velocity after first step")
	_assert_approx(ball.get_variable("height"), 1.0, "Height after first step")
	
	# Test multiple steps to verify free fall
	for i in range(9):  # 9 more steps
		eq_system.solve_step()
	
	# After 10 steps total (t = 0.1s):
	# v = v0 + a*t = 0 + (-9.81)*0.1 = -0.981 m/s
	# h = h0 + v0*t + 0.5*a*t^2 = 1.0 + 0 + 0.5*(-9.81)*0.1^2 = 0.95095 m
	_assert_approx(ball.get_variable("velocity"), -0.981, "Velocity after 10 steps", 1e-3)
	_assert_approx(ball.get_variable("height"), 0.95095, "Height after 10 steps", 1e-5)

func _test_ast_dependencies() -> void:
	_start_test("AST Dependencies")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add variables to component
	comp.add_variable("x")
	comp.add_variable("y")
	comp.add_variable("z")
	
	# Test simple dependency
	eq_system.add_component(comp)
	eq_system.add_equation("test.x = test.y + test.z", comp)
	eq_system.initialize()
	
	# Get the AST node from the last equation
	var tokens = eq_system.tokenize("test.y + test.z")
	var ast_dict = eq_system.parse_expression(tokens)
	var node = ast_dict.node
	
	# Test dependencies
	var deps = node.get_dependencies()
	_assert(deps.has("test.y"), "AST tracks dependency on y")
	_assert(deps.has("test.z"), "AST tracks dependency on z")
	_assert(deps.size() == 2, "AST has correct number of dependencies")
	
	# Test nested dependencies
	tokens = eq_system.tokenize("test.x * (test.y + test.z)")
	ast_dict = eq_system.parse_expression(tokens)
	node = ast_dict.node
	
	deps = node.get_dependencies()
	_assert(deps.has("test.x"), "AST tracks dependency in multiplication")
	_assert(deps.has("test.y"), "AST tracks dependency in nested addition")
	_assert(deps.has("test.z"), "AST tracks dependency in nested addition")
	_assert(deps.size() == 3, "AST has correct number of nested dependencies")
	
	# Test function call dependencies
	tokens = eq_system.tokenize("sin(test.x) + cos(test.y)")
	ast_dict = eq_system.parse_expression(tokens)
	node = ast_dict.node
	
	deps = node.get_dependencies()
	_assert(deps.has("test.x"), "AST tracks dependency in sin function")
	_assert(deps.has("test.y"), "AST tracks dependency in cos function")
	_assert(deps.size() == 2, "AST has correct number of function dependencies")

func _test_ast_differential() -> void:
	_start_test("AST Differential Equations")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add state variables
	comp.add_state_variable("x")
	comp.add_state_variable("v")
	
	# Test simple derivative
	eq_system.add_component(comp)
	eq_system.add_equation("der(test.x) = test.v", comp)
	
	# Get the AST node from the derivative expression
	var tokens = eq_system.tokenize("der(test.x)")
	var ast_dict = eq_system.parse_expression(tokens)
	var node = ast_dict.node
	
	# Test differential properties
	_assert(node.is_differential, "AST identifies derivative function")
	_assert(node.state_variable == "test.x", "AST tracks state variable in derivative")
	
	# Test nested derivative
	tokens = eq_system.tokenize("2.0 * der(test.x) + test.v")
	ast_dict = eq_system.parse_expression(tokens)
	node = ast_dict.node
	
	# The derivative node should be in the left part of the addition
	var der_node = node.left.right  # Navigate to der() node in "2.0 * der(test.x)"
	_assert(der_node.is_differential, "AST identifies derivative in complex expression")
	_assert(der_node.state_variable == "test.x", "AST tracks state variable in complex expression")

func _test_ast_state_variables() -> void:
	_start_test("AST State Variables")
	
	var eq_system = EquationSystem.new()
	var comp = ModelicaComponent.new("test")
	
	# Add state variables and regular variables
	comp.add_state_variable("pos")
	comp.add_state_variable("vel")
	comp.add_variable("force")
	
	# Test state variable tracking in a complex equation
	eq_system.add_component(comp)
	eq_system.add_equation("der(test.pos) = test.vel", comp)
	eq_system.add_equation("der(test.vel) = test.force / 1.0", comp)
	
	# Test first equation
	var tokens = eq_system.tokenize("der(test.pos) = test.vel")
	var ast_dict = eq_system.parse_expression(tokens)
	var node = ast_dict.node
	
	_assert(node.type == "BINARY_OP", "AST creates binary operation for equation")
	_assert(node.left.is_differential, "Left side is marked as differential")
	_assert(node.left.state_variable == "test.pos", "Tracks correct state variable")
	
	# Test second equation with more complex right side
	tokens = eq_system.tokenize("der(test.vel) = test.force / 1.0")
	ast_dict = eq_system.parse_expression(tokens)
	node = ast_dict.node
	
	_assert(node.left.is_differential, "Left side is marked as differential")
	_assert(node.left.state_variable == "test.vel", "Tracks correct state variable")
	
	# Test dependencies in differential equation
	var deps = node.get_dependencies()
	_assert(deps.has("test.force"), "Tracks dependencies in differential equation")
	
	# Test nested derivatives
	tokens = eq_system.tokenize("der(test.vel) + 2.0 * der(test.pos)")
	ast_dict = eq_system.parse_expression(tokens)
	node = ast_dict.node
	
	deps = node.get_dependencies()
	_assert(node.left.is_differential, "First derivative is marked")
	_assert(node.right.right.is_differential, "Second derivative is marked")
	_assert(node.left.state_variable == "test.vel", "First state variable is correct")
	_assert(node.right.right.state_variable == "test.pos", "Second state variable is correct") 