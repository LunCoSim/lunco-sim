@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var MOTOR_FORCE := 50.0  # Forward/backward force
@export var STEERING_FORCE := 30.0  # Turning force
@export var MAX_SPEED := 20.0  # Maximum speed
@export var BRAKE_FORCE := 40.0  # Braking force
@export var DEBUG_MODE := true  # Enable extra debug output

@export_category("Wheel Configuration")
@export var front_left_wheel: Node3D
@export var front_right_wheel: Node3D
@export var back_left_wheel: Node3D
@export var back_right_wheel: Node3D

# Get the parent RigidBody3D node - use the same style as spacecraft controller
@onready var parent: RigidBody3D:
	get:
		return self.get_parent()

# Internal state - simplified like spacecraft controller
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
	print("LCRoverController: MOTOR_FORCE = ", MOTOR_FORCE)

func _on_timer_timeout():
	print("LCRoverController status: authority=", is_multiplayer_authority())
	print("LCRoverController inputs: motor=", motor_input, " steering=", steering_input, " brake=", brake_input)

# Processing physics for Rover controller
func _physics_process(delta: float):
	# Only process if we have authority (same as spacecraft)
	if is_multiplayer_authority():
		if parent:
			# Debug output (only occasionally to avoid spam)
			debug_counter += 1
			if DEBUG_MODE and debug_counter % 30 == 0:
				print("Rover physics: motor_input=", motor_input, " force=", motor_input * MOTOR_FORCE)
				
			# Apply motor force
			var forward_dir = -parent.global_transform.basis.z
			if current_speed < MAX_SPEED:
				var force = forward_dir * motor_input * MOTOR_FORCE
				parent.apply_central_force(force)
				if DEBUG_MODE and debug_counter % 30 == 0:
					print("Applied force: ", force, " mass: ", parent.mass)

			# Apply steering
			var steering_torque = Vector3.UP * (steering_input * STEERING_FORCE)
			parent.apply_torque(steering_torque)
			
			# Apply brakes if needed
			if brake_input > 0:
				var brake_dir = -parent.linear_velocity.normalized()
				var brake_force = brake_dir * (brake_input * BRAKE_FORCE)
				parent.apply_central_force(brake_force)
				brake_applied.emit(brake_input)
			
			# Update wheel rotations
			update_wheels(delta)
			
			# Update speed
			current_speed = parent.linear_velocity.length()
			speed_changed.emit(current_speed)
			
			# Emit other signals
			motor_state_changed.emit(motor_input)
			steering_changed.emit(steering_input)

# Update the wheel visuals
func update_wheels(delta: float):
	# Update wheel rotations and positions
	if front_left_wheel:
		front_left_wheel.rotate_x(current_speed * delta)
	if front_right_wheel:
		front_right_wheel.rotate_x(current_speed * delta)
	if back_left_wheel:
		back_left_wheel.rotate_x(current_speed * delta)
	if back_right_wheel:
		back_right_wheel.rotate_x(current_speed * delta)

# Simple command methods like spacecraft
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)
	if DEBUG_MODE and abs(value) > 0.1:
		print("RoverController: set_motor called with value=", value, " set to ", motor_input)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)
	if DEBUG_MODE and abs(value) > 0.1:
		print("RoverController: set_steering called with value=", value, " set to ", steering_input)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)
	if DEBUG_MODE and value > 0.1:
		print("RoverController: set_brake called with value=", value, " set to ", brake_input)

# Simplified control methods (required for compatibility with signals)
func take_control():
	# Reset all inputs when taking control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	print("RoverController: Control taken")

func release_control():
	# Reset all inputs when releasing control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	print("RoverController: Control released") 
