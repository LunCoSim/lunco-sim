extends Node3D

class_name LCRoverWheel

# Wheel physics parameters
@export var suspension_rest_length := 0.5
@export var suspension_stiffness := 50.0
@export var suspension_damping := 5.0
@export var wheel_radius := 0.4
@export var wheel_friction := 3.0
@export var lateral_friction := 2.0  # Sideways friction for better turning
@export var rolling_resistance := 0.1  # Rolling resistance (lunar dust)

# Internal state
var parent_body: RigidBody3D
var wheel_offset: Vector3
var prev_suspension_length := 0.0
var suspension_force := Vector3.ZERO
var ground_contact := false
var ground_normal := Vector3.UP
var ground_point := Vector3.ZERO
var last_frame_in_contact := false

# Debug
@export var draw_debug := false
var debug_marker: MeshInstance3D

func _ready():
	# Get parent RigidBody3D
	parent_body = get_parent().get_parent()
	
	# Store initial offset from parent's center
	wheel_offset = global_position - parent_body.global_position
	
	# Create debug marker if enabled
	if draw_debug:
		var debug_mesh = SphereMesh.new()
		debug_mesh.radius = 0.1
		debug_marker = MeshInstance3D.new()
		debug_marker.mesh = debug_mesh
		add_child(debug_marker)

func _physics_process(delta):
	# Keep wheel position updated relative to chassis
	apply_suspension_forces(delta)
	if ground_contact:
		apply_friction_forces(delta)
	
	# Apply extra stabilization if we just lost contact with the ground
	if last_frame_in_contact and !ground_contact:
		apply_stabilization_force()
		
	last_frame_in_contact = ground_contact
	
func apply_suspension_forces(delta):
	# Skip if no parent body
	if not parent_body:
		return
	
	# Calculate suspension forces using raycasting
	var ray_start = global_position
	var ray_end = ray_start - global_transform.basis.y * (suspension_rest_length + wheel_radius)
	
	# Create physics parameters for raycast
	var space_state = get_world_3d().direct_space_state
	var ray_params = PhysicsRayQueryParameters3D.new()
	ray_params.from = ray_start
	ray_params.to = ray_end
	ray_params.exclude = [parent_body]
	
	# Perform raycast
	var result = space_state.intersect_ray(ray_params)
	
	if result:
		ground_contact = true
		ground_point = result.position
		ground_normal = result.normal
		
		# Calculate suspension compression
		var ray_length = ray_start.distance_to(ground_point)
		var suspension_length = ray_length - wheel_radius
		
		# Calculate suspension compression ratio (0 = fully extended, 1 = fully compressed)
		var compression = 1.0 - (suspension_length / suspension_rest_length)
		compression = clamp(compression, 0.0, 1.0)
		
		# Calculate suspension force
		var spring_force = compression * suspension_stiffness
		
		# Apply damping
		var velocity_difference = (suspension_length - prev_suspension_length) / delta
		var damping_force = velocity_difference * suspension_damping
		
		# Calculate total force magnitude
		var total_force = spring_force - damping_force
		total_force = max(0, total_force) # Don't allow negative forces
		
		# Create force vector in world space (align with normal)
		suspension_force = ground_normal * total_force
		
		# Apply force to parent body
		parent_body.apply_force(suspension_force, global_position - parent_body.global_position)
		
		# Update debug visualization
		if draw_debug:
			debug_marker.global_position = ground_point
			
		# Store current suspension length for next frame
		prev_suspension_length = suspension_length
	else:
		ground_contact = false
		suspension_force = Vector3.ZERO
		ground_normal = Vector3.UP
		
		# Update debug visualization
		if draw_debug:
			debug_marker.global_position = ray_end

func apply_friction_forces(delta):
	if not ground_contact or not parent_body:
		return
		
	# Get the wheel's velocity at the contact point
	var wheel_velocity = parent_body.linear_velocity + parent_body.angular_velocity.cross(global_position - parent_body.global_position)
	
	# Calculate the velocity direction relative to the wheel orientation
	var forward_dir = -global_transform.basis.z.normalized()
	var side_dir = global_transform.basis.x.normalized()
	
	# Project velocity onto forward and side directions
	var forward_vel_amount = wheel_velocity.dot(forward_dir)
	var forward_velocity = forward_dir * forward_vel_amount
	
	var side_vel_amount = wheel_velocity.dot(side_dir)
	var side_velocity = side_dir * side_vel_amount
	
	# Calculate the friction forces
	var suspension_magnitude = suspension_force.length()
	
	# Calculate rolling resistance (opposing forward motion)
	var rolling_force_mag = forward_vel_amount * rolling_resistance * suspension_magnitude
	var rolling_resistance_force = -forward_dir * rolling_force_mag
	
	# Calculate lateral friction (resisting side sliding)
	var lateral_force_mag = side_vel_amount * lateral_friction * suspension_magnitude
	var lateral_friction_force = -side_dir * lateral_force_mag
	
	# Apply friction forces at the wheel's contact point
	var total_friction = (rolling_resistance_force + lateral_friction_force) * wheel_friction
	parent_body.apply_force(total_friction, global_position - parent_body.global_position)

func apply_stabilization_force():
	# Apply a small stabilization force when wheels come off ground
	# This prevents excessive bouncing and flipping
	if parent_body:
		var up_force = Vector3.UP * 200.0
		parent_body.apply_force(up_force, global_position - parent_body.global_position)

# Returns whether the wheel is in contact with the ground
func is_in_contact() -> bool:
	return ground_contact
	
# Returns the current suspension force magnitude
func get_suspension_force() -> float:
	return suspension_force.length()
	
# Returns the ground normal at the contact point
func get_ground_normal() -> Vector3:
	return ground_normal 