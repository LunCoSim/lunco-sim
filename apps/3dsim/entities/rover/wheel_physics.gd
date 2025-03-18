extends Node3D

class_name LCRoverWheel

# Wheel physics parameters
@export var suspension_rest_length: float = 0.8
@export var suspension_stiffness: float = 800.0
@export var suspension_damping: float = 150.0
@export var wheel_radius: float = 0.4
@export var wheel_friction: float = 0.8
@export var wheel_angular_damp: float = 0.5

# Internal state
var parent_body: RigidBody3D
var ground_contact := false
var ground_normal := Vector3.UP
var ground_point := Vector3.ZERO

# Debug
@export var draw_debug := false
var debug_marker: MeshInstance3D

func _ready():
	# Get parent RigidBody3D
	parent_body = get_parent().get_parent()
	
	# Create debug marker if enabled
	if draw_debug:
		var debug_mesh = SphereMesh.new()
		debug_mesh.radius = 0.1
		debug_marker = MeshInstance3D.new()
		debug_marker.mesh = debug_mesh
		add_child(debug_marker)

func _physics_process(delta):
	# Check for ground contact using raycasting
	check_ground_contact()
	
	# Apply suspension forces if in contact with ground
	if ground_contact:
		apply_suspension_force(delta)

func check_ground_contact():
	# Skip if no parent body
	if not parent_body:
		ground_contact = false
		return
	
	# Calculate ray start and end points
	var ray_start = global_position
	var ray_end = ray_start - global_transform.basis.y * (suspension_rest_length + wheel_radius)
	
	# Setup physics raycast
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
		
		# Update debug visualization
		if draw_debug:
			debug_marker.global_position = ground_point
	else:
		ground_contact = false
		ground_normal = Vector3.UP
		
		# Update debug visualization
		if draw_debug:
			debug_marker.global_position = ray_end

func apply_suspension_force(delta: float):
	if not ground_contact or not parent_body:
		return
	
	# Calculate suspension compression
	var ray_length = global_position.distance_to(ground_point)
	var suspension_length = ray_length - wheel_radius
	
	# Calculate suspension compression ratio (0 = fully extended, 1 = fully compressed)
	var compression = 1.0 - (suspension_length / suspension_rest_length)
	compression = clamp(compression, 0.0, 1.0)
	
	# Calculate spring force
	var spring_force = compression * suspension_stiffness
	
	# Calculate damping force based on vertical velocity
	var relative_velocity = parent_body.linear_velocity.dot(ground_normal)
	var damping_force = -relative_velocity * suspension_damping
	
	# Create force vector in world space combining spring and damping
	var suspension_force = ground_normal * (spring_force + damping_force)
	
	# Apply additional downward force to improve ground contact
	var down_force = ground_normal * (-wheel_friction * 150.0)
	suspension_force += down_force
	
	# Apply force to parent body
	parent_body.apply_force(suspension_force, global_position - parent_body.global_position)

# Returns whether the wheel is in contact with the ground
func is_in_contact() -> bool:
	return ground_contact

# Returns the ground normal at the contact point
func get_ground_normal() -> Vector3:
	return ground_normal 