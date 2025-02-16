@icon("res://controllers/rover/rover.svg")
class_name LC3DSimRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var MOTOR_FORCE := 2000.0  # Forward/backward force (adjusted for lunar gravity)
@export var STEERING_FORCE := 1000.0  # Turning force
@export var MAX_SPEED := 10.0  # Maximum speed (m/s)
@export var BRAKE_FORCE := 1500.0  # Braking force
@export var TRACTION_SLIP := 0.2  # Wheel slip factor (adjusted for lunar surface)

@export_category("Wheel Configuration")
@export var front_left_wheel: Node3D
@export var front_right_wheel: Node3D
@export var back_left_wheel: Node3D
@export var back_right_wheel: Node3D

# Get the parent RigidBody3D node
@onready var parent: RigidBody3D = get_parent()

# Internal state
var motor_input := 0.0  # Range: -1.0 to 1.0
var steering_input := 0.0  # Range: -1.0 to 1.0
var brake_input := 0.0  # Range: 0.0 to 1.0
var current_speed := 0.0
var is_controlled := false

# Signals for UI and effects
signal motor_state_changed(power: float)
signal steering_changed(angle: float)
signal speed_changed(speed: float)
signal brake_applied(force: float)

func _ready():
	# Ensure we have all required components
	if not parent:
		push_warning("RoverController: No RigidBody3D parent found!")
		return
		
	# Setup wheel references if not set
	if not front_left_wheel:
		front_left_wheel = get_node_or_null("../Wheels/FrontLeftWheel")
	if not front_right_wheel:
		front_right_wheel = get_node_or_null("../Wheels/FrontRightWheel")
	if not back_left_wheel:
		back_left_wheel = get_node_or_null("../Wheels/BackLeftWheel")
	if not back_right_wheel:
		back_right_wheel = get_node_or_null("../Wheels/BackRightWheel")

func _physics_process(delta: float):
	if not is_multiplayer_authority():
		return
		
	if not parent:
		return
		
	apply_motor_forces(delta)
	apply_steering(delta)
	apply_brakes(delta)
	update_wheels(delta)
	
	# Update current speed
	current_speed = parent.linear_velocity.length()
	speed_changed.emit(current_speed)

func apply_motor_forces(delta: float):
	# Calculate the forward force based on motor input
	var forward_dir = -parent.global_transform.basis.z
	var target_force = forward_dir * (motor_input * MOTOR_FORCE)
	
	# Apply speed limiting
	if current_speed < MAX_SPEED:
		parent.apply_central_force(target_force)
	
	motor_state_changed.emit(motor_input)

func apply_steering(delta: float):
	# Apply torque for steering, taking into account current speed for better control
	var speed_factor = clamp(current_speed / MAX_SPEED, 0.1, 1.0)
	var steering_torque = Vector3.UP * (steering_input * STEERING_FORCE * speed_factor)
	parent.apply_torque(steering_torque)
	
	steering_changed.emit(steering_input)

func apply_brakes(delta: float):
	if brake_input > 0:
		# Apply brake force opposite to current velocity
		var brake_dir = -parent.linear_velocity.normalized()
		var brake_force = brake_dir * (brake_input * BRAKE_FORCE)
		parent.apply_central_force(brake_force)
		
		brake_applied.emit(brake_input)

func update_wheels(delta: float):
	# Calculate wheel rotation based on current speed
	var wheel_rotation = current_speed * delta * 2.0  # Multiply by 2 for visual effect
	
	# Update wheel rotations
	if front_left_wheel:
		front_left_wheel.rotate_x(wheel_rotation)
	if front_right_wheel:
		front_right_wheel.rotate_x(wheel_rotation)
	if back_left_wheel:
		back_left_wheel.rotate_x(wheel_rotation)
	if back_right_wheel:
		back_right_wheel.rotate_x(wheel_rotation)

# Command methods to control the rover
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0)

# Avatar control methods
func take_control():
	is_controlled = true
	# Reset all inputs when taking control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0

func release_control():
	is_controlled = false
	# Reset all inputs when releasing control
	motor_input = 0.0
	steering_input = 0.0
	brake_input = 0.0 
