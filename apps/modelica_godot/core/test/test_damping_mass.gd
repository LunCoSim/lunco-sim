extends SceneTree

const ModelManager = preload("res://apps/modelica_godot/core/model_manager.gd")
const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

const GRAVITY = 9.81  # m/s²

var test_root: Node
var model_manager: ModelManager = null
var parser: MOParser = null

func _init():
	print("Starting damping mass tests...")
	test_root = Node.new()
	get_root().add_child(test_root)
	_run_tests()
	quit()

func _run_tests():
	_setup()
	_test_damping_mass_model()
	_teardown()
	print("Tests completed.")

func _setup():
	print("Setting up test environment...")
	model_manager = ModelManager.new()
	test_root.add_child(model_manager)
	print("Initializing model manager...")
	model_manager.initialize()
	print("Model manager initialized")
	
	print("Checking package manager...")
	if model_manager.has_package("Mechanical"):
		print("Found Mechanical package")
	else:
		print("Warning: Mechanical package not found")
	
	parser = MOParser.new()
	test_root.add_child(parser)
	print("Setup complete.")

func _teardown():
	print("Cleaning up...")
	if model_manager:
		model_manager.queue_free()
		model_manager = null
	if parser:
		parser.queue_free()
		parser = null
	if test_root:
		test_root.queue_free()
		test_root = null
	print("Cleanup complete.")

func _test_damping_mass_model():
	print("Testing damping mass model...")
	
	# Load and parse model
	var model_path = "res://apps/modelica_godot/components/Mechanical/DampingMassTest.mo"
	var file = FileAccess.open(model_path, FileAccess.READ)
	if not file:
		push_error("Model file does not exist")
		return
	
	var content = file.get_as_text()
	print("Model content loaded")
	
	# Parse model
	var model_data = parser.parse_text(content)
	if model_data.is_empty():
		push_error("Failed to parse model")
		return
	
	print("Model parsed successfully")
	print("Model type:", model_data.type)
	print("Model name:", model_data.name)
	
	# Add model to manager
	model_manager._add_model_to_tree(model_data)
	
	# Run simulation
	var dt = 0.01  # Smaller time step for better accuracy
	var simulation_time = 5.0  # Run for 5 seconds
	var steps = int(simulation_time / dt)
	var time = 0.0
	var positions = []
	var velocities = []
	
	print("\nRunning simulation...")
	
	# Initial conditions from model
	var initial_position = 1.0  # x0 from model
	var initial_velocity = 0.0  # v0 from model
	var mass = 1.0             # mass.m from model
	var damping = 0.5          # damper.d from model
	
	# Calculate theoretical terminal velocity
	var terminal_velocity = GRAVITY * mass / damping
	print("Initial conditions:")
	print("Position: %.3f m" % initial_position)
	print("Velocity: %.3f m/s" % initial_velocity)
	print("Mass: %.3f kg" % mass)
	print("Damping: %.3f N⋅s/m" % damping)
	print("Theoretical terminal velocity: %.3f m/s" % terminal_velocity)
	print("\nSimulation steps:")
	
	var current_position = initial_position
	var current_velocity = initial_velocity
	var last_velocity_change = 0.0
	
	for i in range(steps):
		time += dt
		
		# Store previous velocity for acceleration calculation
		var prev_velocity = current_velocity
		
		# Compute forces
		var gravity_force = mass * GRAVITY
		var damping_force = -damping * current_velocity
		var total_force = gravity_force + damping_force
		
		# Compute acceleration (F = ma)
		var acceleration = total_force / mass
		
		# Update velocity and position using semi-implicit Euler
		current_velocity += acceleration * dt
		current_position -= current_velocity * dt  # Negative because positive is upward
		
		# Calculate velocity change rate
		last_velocity_change = abs(current_velocity - prev_velocity) / dt
		
		positions.append(current_position)
		velocities.append(current_velocity)
		
		if i % (steps/10) == 0:  # Print 10 evenly spaced steps
			print("Time: %.2f, Position: %.3f, Velocity: %.3f, Accel: %.3f" % 
				[time, current_position, current_velocity, acceleration])
	
	# Verify damping behavior
	print("\nVerifying damping behavior...")
	
	# 1. Check if position is decreasing (moving downward)
	if positions[-1] >= initial_position:
		push_error("Position should decrease due to gravity and damping")
		return
	print("✓ Position decreases as expected")
	
	# 2. Check if velocity is approaching terminal velocity
	var final_velocity = velocities[-1]
	var velocity_diff = abs(final_velocity - terminal_velocity)
	var velocity_error = velocity_diff / terminal_velocity * 100
	print("Final velocity: %.3f m/s (%.1f%% of terminal velocity)" % 
		[final_velocity, (final_velocity/terminal_velocity * 100)])
	print("Velocity change rate: %.3f m/s²" % last_velocity_change)
	
	# Allow for 20% difference from terminal velocity
	if velocity_error > 20:
		push_error("Velocity should be within 20% of terminal velocity")
		return
	print("✓ Velocity approaches terminal velocity (%.3f m/s)" % terminal_velocity)
	
	# 3. Check energy dissipation rate
	var energy_decreasing = true
	for i in range(1, positions.size()):
		var prev_energy = 0.5 * mass * velocities[i-1] * velocities[i-1] + mass * GRAVITY * positions[i-1]
		var curr_energy = 0.5 * mass * velocities[i] * velocities[i] + mass * GRAVITY * positions[i]
		if curr_energy > prev_energy:
			energy_decreasing = false
			break
	
	if not energy_decreasing:
		push_error("Total energy should decrease due to damping")
		return
	print("✓ Energy dissipation verified")
	
	print("\nAll tests passed successfully") 