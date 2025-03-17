@icon("res://controllers/rover/rover.svg")
class_name LC3DSimRoverController
extends LCController

# Export categories for easy configuration in the editor
@export_category("Rover Movement Parameters")
@export var MOTOR_FORCE := 4000.0  # Forward/backward force (adjusted for lunar gravity)
@export var STEERING_FORCE := 2000.0  # Turning force
@export var MAX_SPEED := 12.0  # Maximum speed (m/s)
@export var BRAKE_FORCE := 2500.0  # Braking force
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

# State tracking
var wheels_on_ground := 0
var ground_contact := false

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
	
	# Update ground contact state
	check_ground_contact()
	
	# Apply forces
	apply_motor_forces(delta)
	apply_steering(delta)
	apply_brakes(delta)
	update_wheel_visual_rotation(delta)
	
	# Update current speed
	current_speed = parent.linear_velocity.length()
	speed_changed.emit(current_speed)

func check_ground_contact():
	# Count how many wheels are on the ground
	wheels_on_ground = 0
	
	if has_ground_contact(front_left_wheel):
		wheels_on_ground += 1
	if has_ground_contact(front_right_wheel):
		wheels_on_ground += 1
	if has_ground_contact(back_left_wheel):
		wheels_on_ground += 1
	if has_ground_contact(back_right_wheel):
		wheels_on_ground += 1
	
	# Vehicle has traction if at least one wheel is on the ground
	ground_contact = wheels_on_ground > 0

func has_ground_contact(wheel: Node3D) -> bool:
	if not wheel:
		return false
	
	if wheel.has_method("is_in_contact"):
		return wheel.is_in_contact()
	
	return false

func apply_motor_forces(delta: float):
	# Skip if we don't have any ground contact
	if not ground_contact:
		return
	
	# Get forward direction from vehicle's orientation
	var forward_dir = -parent.global_transform.basis.z.normalized()
	
	# Calculate motor force
	var target_force = forward_dir * (motor_input * MOTOR_FORCE)
	
	# Apply at the wheel contact points
	for wheel in [front_left_wheel, front_right_wheel, back_left_wheel, back_right_wheel]:
		if has_ground_contact(wheel):
			# Apply a portion of the force at each wheel
			var wheel_force = target_force * (1.0 / max(1, wheels_on_ground))
			parent.apply_force(wheel_force, wheel.global_position - parent.global_position)
	
	motor_state_changed.emit(motor_input)

func apply_steering(delta: float):
	# Only apply steering when on the ground
	if not ground_contact:
		return
	
	# Scale steering based on speed (easier turning at lower speeds)
	var speed_factor = clamp(remap(current_speed, 0, MAX_SPEED, 1.0, 0.5), 0.5, 1.0)
	var steering_torque = Vector3.UP * (steering_input * STEERING_FORCE * speed_factor)
	
	# Apply torque for steering
	parent.apply_torque(steering_torque)
	
	steering_changed.emit(steering_input)

func apply_brakes(delta: float):
	if brake_input > 0:
		# Create brake force opposite to current velocity
		if parent.linear_velocity.length_squared() > 0.1:
			var brake_dir = -parent.linear_velocity.normalized()
			var brake_force = brake_dir * (brake_input * BRAKE_FORCE)
			
			# Apply central brake force for stability
			parent.apply_central_force(brake_force * 1.5)
			
			# Apply at wheels for more authentic braking
			for wheel in [front_left_wheel, front_right_wheel, back_left_wheel, back_right_wheel]:
				if has_ground_contact(wheel):
					parent.apply_force(brake_force * 0.25, wheel.global_position - parent.global_position)
			
			# Apply damping to reduce spinning
			parent.apply_torque(-parent.angular_velocity * BRAKE_FORCE * 0.2)
		
		brake_applied.emit(brake_input)

func update_wheel_visual_rotation(delta: float):
	# Calculate rotation speed - based on the parent's forward velocity
	var forward_dir = -parent.global_transform.basis.z
	var forward_velocity = parent.linear_velocity.dot(forward_dir)
	var wheel_rotation_speed = forward_velocity * delta * 2.0
	
	# Update the visual rotation of all wheels
	rotate_wheel(front_left_wheel, wheel_rotation_speed)
	rotate_wheel(front_right_wheel, wheel_rotation_speed)
	rotate_wheel(back_left_wheel, wheel_rotation_speed)
	rotate_wheel(back_right_wheel, wheel_rotation_speed)

func rotate_wheel(wheel: Node3D, amount: float):
	if wheel:
		var wheel_mesh = wheel.get_node_or_null("MeshInstance3D")
		if wheel_mesh:
			wheel_mesh.rotate_x(amount)

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
