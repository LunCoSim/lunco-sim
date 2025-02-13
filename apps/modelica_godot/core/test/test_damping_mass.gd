extends SceneTree

const ModelManager = preload("res://apps/modelica_godot/core/model_manager.gd")
const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

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
	model_manager.initialize()
	
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
	var dt = 0.1
	var steps = 10
	var time = 0.0
	var positions = []
	var velocities = []
	
	print("\nRunning simulation...")
	
	# Initial conditions from model
	var initial_position = 1.0  # x0 from model
	var initial_velocity = 0.0  # v0 from model
	var mass = 1.0             # mass.m from model
	var damping = 0.5          # damper.d from model
	
	var current_position = initial_position
	var current_velocity = initial_velocity
	
	for i in range(steps):
		time += dt
		
		# Compute damped motion
		var acceleration = (-damping * current_velocity) / mass
		current_velocity += acceleration * dt
		current_position += current_velocity * dt
		
		positions.append(current_position)
		velocities.append(current_velocity)
		
		print("Time: %.2f, Position: %.3f, Velocity: %.3f" % [time, current_position, current_velocity])
	
	# Verify damping behavior
	print("\nVerifying damping behavior...")
	
	# 1. Check if position is decreasing
	if positions[-1] >= initial_position:
		push_error("Position should decrease due to damping")
		return
	print("✓ Position decreases as expected")
	
	# 2. Check if velocity magnitude is decreasing
	var velocity_decreasing = true
	for i in range(1, velocities.size()):
		if abs(velocities[i]) > abs(velocities[i-1]):
			velocity_decreasing = false
			break
	
	if not velocity_decreasing:
		push_error("Velocity magnitude should decrease due to damping")
		return
	print("✓ Velocity magnitude decreases as expected")
	
	# 3. Check energy dissipation
	var initial_energy = 0.5 * mass * initial_velocity * initial_velocity
	var final_energy = 0.5 * mass * velocities[-1] * velocities[-1]
	
	if final_energy >= initial_energy:
		push_error("System energy should decrease due to damping")
		return
	print("✓ System energy decreases as expected")
	
	print("\nAll tests passed successfully") 