@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var ENGINE_FORCE := 3000.0  # Balanced engine force for good acceleration
@export var STEERING_FORCE := 0.5  # Balanced steering for good handling
@export var MAX_SPEED := 5.0  # Realistic max speed for lunar rover
@export var BRAKE_FORCE := 1000.0  # Increased braking force for better control
@export var DEBUG_MODE := true  # Enable extra debug output

# Get the parent VehicleBody3D node
@onready var parent: VehicleBody3D:
	get:
		var p = self.get_parent()
		if p and p is VehicleBody3D:
			return p
		else:
			push_error("RoverController: Parent is not a VehicleBody3D! Got: " + str(p))
			return null

# Internal state
var motor_input := 0.0
var steering_input := 0.0
var brake_input := 0.0
var current_speed := 0.0

var debug_counter := 0

# Signals
signal motor_state_changed(power: float)
signal steering_changed(angle: float)
signal speed_changed(speed: float)
signal brake_applied(force: float)

# Initialize the controller
func _ready():
	print("LCRoverController: Initializing node ", name)
	
	# Ensure we're in the right group for discovery
	if not is_in_group("RoverControllers"):
		add_to_group("RoverControllers")
	
	# Reset inputs on start
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Create a timer to periodically report status
	var timer = Timer.new()
	timer.wait_time = 3.0
	timer.one_shot = false
	timer.autostart = true
	timer.connect("timeout", Callable(self, "_on_timer_timeout"))
	add_child(timer)
	
	print("LCRoverController: Initialized with parent: ", parent.name)
	print("LCRoverController: ENGINE_FORCE = ", ENGINE_FORCE)
	print("LCRoverController: Initial multiplayer authority = ", is_multiplayer_authority())
	
	# Ensure parent is a VehicleBody3D
	if not parent is VehicleBody3D:
		push_error("RoverController's parent must be a VehicleBody3D")
	else:
		# Directly set initial values
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
		print("RoverController: Vehicle properties initialized")

# Add a function to debug physics state
func debug_physics_state():
	if parent and parent is VehicleBody3D:
		print("--- ROVER PHYSICS DEBUG ---")
		print("Position: ", parent.global_position)
		print("Linear velocity: ", parent.linear_velocity)
		print("Engine force: ", parent.engine_force)
		print("Gravity scale: ", parent.gravity_scale)
		print("Is on floor: ", parent.is_on_floor() if parent.has_method("is_on_floor") else "N/A")
		
		# Debug wheel contact
		for wheel in parent.get_children():
			if wheel is VehicleWheel3D:
				print("Wheel ", wheel.name, " contact: ", wheel.is_in_contact())
				
		print("-------------------------")
		
func _on_timer_timeout():
	print("LCRoverController status: authority=", is_multiplayer_authority())
	print("LCRoverController inputs: motor=", motor_input, " steering=", steering_input, " brake=", brake_input)
	if parent:
		print("Rover parent values: engine_force=", parent.engine_force, " steering=", parent.steering, " brake=", parent.brake)
		print("Rover speed: ", current_speed)
		
		# Add physics debugging
		debug_physics_state()

# Processing physics for Rover controller
func _physics_process(_delta: float):
	# Process regardless of authority to ensure controls always work
	if parent and parent is VehicleBody3D:
		# Debug output (only occasionally to avoid spam)
		debug_counter += 1
		if DEBUG_MODE and debug_counter % 30 == 0:
			print("Rover physics: motor_input=", motor_input, " engine_force=", motor_input * ENGINE_FORCE)
			print("Rover parent direct values: engine_force=", parent.engine_force, " steering=", parent.steering)
			
		# Apply engine force to VehicleBody3D
		parent.engine_force = motor_input * ENGINE_FORCE
		
		# Apply steering to VehicleBody3D - note the negative sign to fix wheel direction
		parent.steering = -steering_input * STEERING_FORCE  
		
		# Apply brakes if needed
		parent.brake = brake_input * BRAKE_FORCE
		if brake_input > 0:
			brake_applied.emit(brake_input)
		
		# Update speed
		current_speed = parent.linear_velocity.length()
		speed_changed.emit(current_speed)
		
		# Emit other signals
		motor_state_changed.emit(motor_input)
		steering_changed.emit(steering_input)

# Simple command methods
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)
	# Immediately apply engine force if we have a parent
	if parent and parent is VehicleBody3D:
		parent.engine_force = motor_input * ENGINE_FORCE
	
	if DEBUG_MODE and abs(value) > 0.1:
		print("RoverController: set_motor called with value=", value, " set to ", motor_input)
		if parent and parent is VehicleBody3D:
			print("  - Direct engine_force set to: ", parent.engine_force)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)
	# Immediately apply steering if we have a parent - fix wheel direction with negative sign
	if parent and parent is VehicleBody3D:
		parent.steering = -steering_input * STEERING_FORCE
	
	if DEBUG_MODE and abs(value) > 0.1:
		print("RoverController: set_steering called with value=", value, " set to ", steering_input)
		if parent and parent is VehicleBody3D:
			print("  - Direct steering set to: ", parent.steering)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)
	# Immediately apply brake if we have a parent
	if parent and parent is VehicleBody3D:
		parent.brake = brake_input * BRAKE_FORCE
	
	if DEBUG_MODE and value > 0.1:
		print("RoverController: set_brake called with value=", value, " set to ", brake_input)
		if parent and parent is VehicleBody3D:
			print("  - Direct brake set to: ", parent.brake)

# Simplified control methods (required for compatibility with signals)
func take_control():
	# Reset all inputs when taking control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Make sure parent values are reset too
	if parent and parent is VehicleBody3D:
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
	
	print("RoverController: Control taken")

func release_control():
	# Reset all inputs when releasing control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	# Make sure parent values are reset too
	if parent and parent is VehicleBody3D:
		parent.engine_force = 0.0
		parent.steering = 0.0
		parent.brake = 0.0
	
	print("RoverController: Control released")

# Add a regular process function to regularly check input values
func _process(_delta: float):
	if parent and parent is VehicleBody3D:
		# Check if motor values are being applied consistently
		if DEBUG_MODE and debug_counter % 60 == 0:
			print("RoverController _process: Input values - motor:", motor_input, 
				" steering:", steering_input, 
				" brake:", brake_input)
			print("RoverController vehicle values - engine:", parent.engine_force, 
				" steering:", parent.steering,
				" brake:", parent.brake)
			print("RoverController speed:", current_speed)
			
	# Always test direct input from keyboard for debugging
	var debug_motor = Input.get_action_strength("move_forward") - Input.get_action_strength("move_backward")
	if abs(debug_motor) > 0.1 and DEBUG_MODE and debug_counter % 60 == 0:
		print("RoverController direct keyboard input:", debug_motor) 
