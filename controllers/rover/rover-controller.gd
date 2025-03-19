@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var MOTOR_FORCE := 50.0  # Forward/backward force
@export var STEERING_FORCE := 30.0  # Turning force
@export var MAX_SPEED := 20.0  # Maximum speed
@export var BRAKE_FORCE := 40.0  # Braking force

@export_category("Wheel Configuration")
@export var front_left_wheel: Node3D
@export var front_right_wheel: Node3D
@export var back_left_wheel: Node3D
@export var back_right_wheel: Node3D

# Get the parent RigidBody3D node
@onready var parent: RigidBody3D:
	get:
		return self.get_parent()

# Internal state - simplified like spacecraft controller
var motor_input := 0.0
var steering_input := 0.0
var brake_input := 0.0
var current_speed := 0.0

# Signals
signal motor_state_changed(power: float)
signal steering_changed(angle: float)
signal speed_changed(speed: float)
signal brake_applied(force: float)

# Initialize the controller
func _ready():
	print("LCRoverController: Initializing node ", name)
	
	# Ensure we're in the right group
	if not is_in_group("RoverControllers"):
		add_to_group("RoverControllers")
		
	# Ensure we have connections to control signals
	if parent.has_signal("control_granted"):
		if not parent.control_granted.is_connected(take_control):
			parent.control_granted.connect(take_control)
	
	if parent.has_signal("control_released"):
		if not parent.control_released.is_connected(release_control):
			parent.control_released.connect(release_control)
	
	# Reset inputs on start
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0
	
	print("LCRoverController: Initialized")

# Processing physics for Rover controller
func _physics_process(delta: float):
	# Only process when we have authority (exactly like spacecraft)
	if is_multiplayer_authority():
		if parent:
			# Apply motor force
			var forward_dir = -parent.global_transform.basis.z
			if current_speed < MAX_SPEED:
				parent.apply_central_force(forward_dir * motor_input * MOTOR_FORCE)
			
			# Apply steering
			parent.apply_torque(Vector3.UP * (steering_input * STEERING_FORCE))
			
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

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)

# Handle control signals, but keep them simple
func take_control():
	print("RoverController: Control taken")
	# Reset all inputs when taking control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0

func release_control():
	print("RoverController: Control released")
	# Reset all inputs when releasing control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0 
