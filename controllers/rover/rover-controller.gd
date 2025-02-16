@icon("res://controllers/rover/rover.svg")
class_name LCRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var MOTOR_FORCE := 50.0  # Forward/backward force
@export var STEERING_FORCE := 30.0  # Turning force
@export var MAX_SPEED := 20.0  # Maximum speed
@export var BRAKE_FORCE := 40.0  # Braking force
@export var TRACTION_SLIP := 0.1  # Wheel slip factor

@export_category("Wheel Configuration")
@export var front_left_wheel: Node3D
@export var front_right_wheel: Node3D
@export var back_left_wheel: Node3D
@export var back_right_wheel: Node3D

# Get the parent RigidBody3D node
@onready var parent: RigidBody3D:
	get:
		return self.get_parent()

# Internal state
var motor_input := 0.0  # Range: -1.0 to 1.0
var steering_input := 0.0  # Range: -1.0 to 1.0
var brake_input := 0.0  # Range: 0.0 to 1.0
var current_speed := 0.0

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
	# Apply torque for steering
	var steering_torque = Vector3.UP * (steering_input * STEERING_FORCE)
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
	# Update wheel rotations and positions
	if front_left_wheel:
		front_left_wheel.rotate_x(current_speed * delta)
	if front_right_wheel:
		front_right_wheel.rotate_x(current_speed * delta)
	if back_left_wheel:
		back_left_wheel.rotate_x(current_speed * delta)
	if back_right_wheel:
		back_right_wheel.rotate_x(current_speed * delta)

# Command methods to control the rover
func set_motor(value: float):
	motor_input = clamp(value, -1.0, 1.0)

func set_steering(value: float):
	steering_input = clamp(value, -1.0, 1.0)

func set_brake(value: float):
	brake_input = clamp(value, 0.0, 1.0) 