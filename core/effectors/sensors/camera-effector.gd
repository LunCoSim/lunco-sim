class_name LCCameraEffector
extends LCSensorEffector

## Camera sensor effector for vision-based robotics.
##
## Provides image-based measurements including object detection,
## depth estimation, and feature tracking.

@export_group("Camera Configuration")
@export var resolution: Vector2i = Vector2i(640, 480)  ## Image resolution
@export var field_of_view: float = 60.0  ## Horizontal FOV in degrees
@export var near_plane: float = 0.1  ## Near clipping plane in meters
@export var far_plane: float = 100.0  ## Far clipping plane in meters

@export_group("Camera Features")
@export var enable_depth: bool = false  ## Enable depth sensing
@export var enable_object_detection: bool = false  ## Enable object detection
@export var detection_range: float = 50.0  ## Object detection range in meters

@export_group("Image Quality")
@export var exposure: float = 1.0  ## Exposure level (0.1 to 10.0)
@export var add_motion_blur: bool = false  ## Simulate motion blur
@export var add_lens_distortion: bool = false  ## Simulate lens distortion

# Measurement data
var visible_objects: Array[Dictionary] = []  ## Detected objects in view
var depth_map: Array[float] = []  ## Depth values (if enabled)
var image_center_depth: float = 0.0  ## Depth at image center

# Internal
var camera_3d: Camera3D
var space_state: PhysicsDirectSpaceState3D

func _ready():
	super._ready()
	mass = 0.3  # Typical camera mass
	power_consumption = 3.0 + (2.0 if enable_depth else 0.0)
	
	# Create internal camera for frustum calculations
	camera_3d = Camera3D.new()
	add_child(camera_3d)
	camera_3d.fov = field_of_view
	camera_3d.near = near_plane
	camera_3d.far = far_plane
	
	space_state = get_world_3d().direct_space_state

func _update_measurement():
	if not space_state:
		space_state = get_world_3d().direct_space_state
		if not space_state:
			is_valid = false
			return
	
	visible_objects.clear()
	
	# Detect objects in camera view
	if enable_object_detection:
		_detect_objects()
	
	# Measure depth at center
	if enable_depth:
		_measure_center_depth()
	
	measurement = {
		"visible_objects": visible_objects,
		"depth": image_center_depth,
		"object_count": visible_objects.size()
	}

## Detects objects in the camera's field of view.
func _detect_objects():
	# Get all bodies in detection range
	var query = PhysicsShapeQueryParameters3D.new()
	var sphere = SphereShape3D.new()
	sphere.radius = detection_range
	query.shape = sphere
	query.transform = global_transform
	query.exclude = [get_parent()]
	
	var results = space_state.intersect_shape(query, 100)
	
	for result in results:
		var collider = result.collider
		if not collider:
			continue
		
		# Check if object is in camera frustum
		var obj_pos = collider.global_position
		if _is_in_frustum(obj_pos):
			var detection = _create_detection(collider, obj_pos)
			visible_objects.append(detection)

## Checks if a point is in the camera frustum.
func _is_in_frustum(world_pos: Vector3) -> bool:
	if not camera_3d:
		return false
	
	return camera_3d.is_position_in_frustum(world_pos)

## Creates a detection dictionary for an object.
func _create_detection(collider: Node3D, world_pos: Vector3) -> Dictionary:
	var local_pos = global_transform.inverse() * world_pos
	var distance = global_position.distance_to(world_pos)
	
	# Add noise to distance
	if add_noise:
		distance = add_gaussian_noise_custom(distance, noise_std_dev)
	
	# Calculate bearing (azimuth and elevation)
	var bearing = _calculate_bearing(local_pos)
	
	# Add angular noise
	if add_noise:
		bearing.x = add_gaussian_noise_custom(bearing.x, 0.5)  # 0.5 degree noise
		bearing.y = add_gaussian_noise_custom(bearing.y, 0.5)
	
	return {
		"object": collider,
		"name": collider.name,
		"distance": distance,
		"azimuth": bearing.x,  # Horizontal angle in degrees
		"elevation": bearing.y,  # Vertical angle in degrees
		"position_local": local_pos,
		"position_world": world_pos,
		"in_center": _is_near_center(bearing)
	}

## Calculates bearing (azimuth, elevation) to a local position.
func _calculate_bearing(local_pos: Vector3) -> Vector2:
	var azimuth = rad_to_deg(atan2(local_pos.x, -local_pos.z))
	var elevation = rad_to_deg(atan2(local_pos.y, sqrt(local_pos.x * local_pos.x + local_pos.z * local_pos.z)))
	return Vector2(azimuth, elevation)

## Checks if bearing is near image center.
func _is_near_center(bearing: Vector2, threshold: float = 5.0) -> bool:
	return abs(bearing.x) < threshold and abs(bearing.y) < threshold

## Measures depth at the center of the image.
func _measure_center_depth():
	var ray_origin = global_position
	var ray_direction = -global_transform.basis.z  # Forward direction
	var ray_end = ray_origin + ray_direction * far_plane
	
	var query = PhysicsRayQueryParameters3D.create(ray_origin, ray_end)
	query.exclude = [get_parent()]
	
	var result = space_state.intersect_ray(query)
	
	if result:
		image_center_depth = ray_origin.distance_to(result.position)
		if add_noise:
			image_center_depth = add_gaussian_noise_custom(image_center_depth, noise_std_dev)
	else:
		image_center_depth = far_plane

## Returns objects detected in the current frame.
func get_visible_objects() -> Array[Dictionary]:
	return visible_objects if is_valid else []

## Returns the closest detected object.
func get_closest_object() -> Dictionary:
	if visible_objects.is_empty():
		return {}
	
	var closest = visible_objects[0]
	for obj in visible_objects:
		if obj.distance < closest.distance:
			closest = obj
	
	return closest

## Returns objects near the image center.
func get_centered_objects(threshold: float = 5.0) -> Array[Dictionary]:
	var centered: Array[Dictionary] = []
	for obj in visible_objects:
		if obj.in_center:
			centered.append(obj)
	return centered

## Projects a world position to image coordinates (normalized 0-1).
func world_to_image(world_pos: Vector3) -> Vector2:
	if not camera_3d:
		return Vector2(-1, -1)
	
	var local_pos = camera_3d.global_transform.inverse() * world_pos
	
	# Simple perspective projection
	if local_pos.z >= 0:  # Behind camera
		return Vector2(-1, -1)
	
	var fov_rad = deg_to_rad(field_of_view)
	var aspect = float(resolution.x) / float(resolution.y)
	
	var x_ndc = local_pos.x / (-local_pos.z * tan(fov_rad / 2.0))
	var y_ndc = local_pos.y / (-local_pos.z * tan(fov_rad / 2.0) / aspect)
	
	# Convert to 0-1 range
	var x_img = (x_ndc + 1.0) / 2.0
	var y_img = (y_ndc + 1.0) / 2.0
	
	return Vector2(x_img, y_img)

func _update_telemetry():
	super._update_telemetry()
	Telemetry["visible_objects"] = visible_objects.size()
	Telemetry["center_depth"] = image_center_depth
	Telemetry["fov"] = field_of_view
	Telemetry["resolution"] = resolution
