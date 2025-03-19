@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var ENGINE_FORCE := 5000.0  # Higher engine force for better response
@export var STEERING_FORCE := 0.5  # Steering force (max angle in radians)
@export var MAX_SPEED := 8.0  # Maximum speed
@export var BRAKE_FORCE := 800.0  # Braking force
@export var DEBUG_MODE := true  # Enable extra debug output

# Get the parent VehicleBody3D node
@onready var parent: VehicleBody3D:
	get:
		return self.get_parent()

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

func _on_timer_timeout():
	print("LCRoverController status: authority=", is_multiplayer_authority())
	print("LCRoverController inputs: motor=", motor_input, " steering=", steering_input, " brake=", brake_input)
	if is_multiplayer_authority() and parent:
		print("Rover parent values: engine_force=", parent.engine_force, " steering=", parent.steering, " brake=", parent.brake)
		print("Rover speed: ", current_speed)

# Processing physics for Rover controller
func _physics_process(_delta: float):
	# TEMPORARY: Process regardless of authority for testing
	# if is_multiplayer_authority():
	if true:  # Process regardless of authority for testing
		if parent and parent is VehicleBody3D:
			# Debug output (only occasionally to avoid spam)
			debug_counter += 1
			if DEBUG_MODE and debug_counter % 30 == 0:
				print("Rover physics: motor_input=", motor_input, " engine_force=", motor_input * ENGINE_FORCE)
				print("Rover parent direct values: engine_force=", parent.engine_force, " steering=", parent.steering)
				print("Rover authority status: ", is_multiplayer_authority())
				
			# Apply engine force to VehicleBody3D
			parent.engine_force = motor_input * ENGINE_FORCE
			
			# Apply steering to VehicleBody3D
			parent.steering = steering_input * STEERING_FORCE  
			
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
	# Immediately apply steering if we have a parent
	if parent and parent is VehicleBody3D:
		parent.steering = steering_input * STEERING_FORCE
	
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
