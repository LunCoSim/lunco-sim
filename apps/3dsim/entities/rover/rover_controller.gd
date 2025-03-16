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

# Wheel physics state
var front_wheels_contact := false
var rear_wheels_contact := false

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
	
	# Update wheel contact states
	update_wheel_contact_states()
	
	# Apply forces
	apply_motor_forces(delta)
	apply_steering(delta)
	apply_brakes(delta)
	update_wheel_visual_rotation(delta)
	
	# Update current speed
	current_speed = parent.linear_velocity.length()
	speed_changed.emit(current_speed)

func update_wheel_contact_states():
	# Check if front and rear wheels are in contact with the ground
	var fl_contact = front_left_wheel.has_method("is_in_contact") and front_left_wheel.is_in_contact()
	var fr_contact = front_right_wheel.has_method("is_in_contact") and front_right_wheel.is_in_contact()
	var bl_contact = back_left_wheel.has_method("is_in_contact") and back_left_wheel.is_in_contact()
	var br_contact = back_right_wheel.has_method("is_in_contact") and back_right_wheel.is_in_contact()
	
	front_wheels_contact = fl_contact or fr_contact
	rear_wheels_contact = bl_contact or br_contact

func apply_motor_forces(delta: float):
	# Only apply motor forces if wheels are in contact with the ground
	if not (front_wheels_contact or rear_wheels_contact):
		return
		
	# Calculate the forward force based on motor input
	var forward_dir = -parent.global_transform.basis.z
	var target_force = forward_dir * (motor_input * MOTOR_FORCE)
	
	# Apply forces at wheel positions for more realistic behavior
	if front_wheels_contact:
		var front_force = target_force * 0.5  # Distribute force between front wheels
		parent.apply_force(front_force, front_left_wheel.global_position - parent.global_position)
		parent.apply_force(front_force, front_right_wheel.global_position - parent.global_position)
	
	if rear_wheels_contact:
		var rear_force = target_force * 0.5  # Distribute force between rear wheels
		parent.apply_force(rear_force, back_left_wheel.global_position - parent.global_position)
		parent.apply_force(rear_force, back_right_wheel.global_position - parent.global_position)
	
	motor_state_changed.emit(motor_input)

func apply_steering(delta: float):
	# Only apply steering if front wheels are in contact
	if not front_wheels_contact:
		# Still allow some air steering but reduced
		var air_steering_torque = Vector3.UP * (steering_input * STEERING_FORCE * 0.2)
		parent.apply_torque(air_steering_torque)
		return
		
	# Apply torque for steering, taking into account current speed for better control
	var speed_factor = clamp(current_speed / MAX_SPEED, 0.1, 1.0)
	var steering_torque = Vector3.UP * (steering_input * STEERING_FORCE * speed_factor)
	parent.apply_torque(steering_torque)
	
	steering_changed.emit(steering_input)

func apply_brakes(delta: float):
	if brake_input > 0:
		# Apply brake force opposite to current velocity at wheel positions
		var brake_dir = -parent.linear_velocity.normalized()
		var brake_force = brake_dir * (brake_input * BRAKE_FORCE)
		
		# Apply stronger central braking force for quicker stopping
		parent.apply_central_force(brake_force * 1.5)
		
		if front_wheels_contact:
			parent.apply_force(brake_force * 0.5, front_left_wheel.global_position - parent.global_position)
			parent.apply_force(brake_force * 0.5, front_right_wheel.global_position - parent.global_position)
			
		if rear_wheels_contact:
			parent.apply_force(brake_force * 0.5, back_left_wheel.global_position - parent.global_position)
			parent.apply_force(brake_force * 0.5, back_right_wheel.global_position - parent.global_position)
		
		# Add angular damping when braking to prevent spinning
		if current_speed < 1.0:
			parent.apply_torque(-parent.angular_velocity * BRAKE_FORCE * 0.5)
		
		brake_applied.emit(brake_input)

func update_wheel_visual_rotation(delta: float):
	# Calculate wheel rotation speed based on rover's velocity
	# Project velocity onto wheel's forward direction
	var wheel_rotation_speed = current_speed * delta * 2.0
	
	# Update wheel rotations along their local X axis (for wheels oriented sideways)
	if front_left_wheel:
		var wheel_mesh = front_left_wheel.get_node_or_null("MeshInstance3D")
		if wheel_mesh:
			# Changed from rotate_z to rotate_x for correct axis
			wheel_mesh.rotate_x(wheel_rotation_speed * sign(motor_input))
	
	if front_right_wheel:
		var wheel_mesh = front_right_wheel.get_node_or_null("MeshInstance3D")
		if wheel_mesh:
			# Changed from rotate_z to rotate_x for correct axis
			wheel_mesh.rotate_x(wheel_rotation_speed * sign(motor_input))
	
	if back_left_wheel:
		var wheel_mesh = back_left_wheel.get_node_or_null("MeshInstance3D")
		if wheel_mesh:
			# Changed from rotate_z to rotate_x for correct axis
			wheel_mesh.rotate_x(wheel_rotation_speed * sign(motor_input))
	
	if back_right_wheel:
		var wheel_mesh = back_right_wheel.get_node_or_null("MeshInstance3D")
		if wheel_mesh:
			# Changed from rotate_z to rotate_x for correct axis
			wheel_mesh.rotate_x(wheel_rotation_speed * sign(motor_input))

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
