class_name LCLidarEffector
extends LCSensorEffector

## Lidar (Light Detection and Ranging) sensor effector.
##
## Performs raycasting to measure distances to objects.
## Supports single-beam, scanning, and 3D point cloud modes.

@export_group("Lidar Configuration")
@export var lidar_mode: LidarMode = LidarMode.SINGLE_BEAM
@export var max_range: float = 100.0  ## Maximum detection range in meters
@export var min_range: float = 0.1  ## Minimum detection range in meters
@export var beam_direction: Vector3 = Vector3(0, 0, -1)  ## Local beam direction

@export_group("Scanning Configuration")
@export var horizontal_fov: float = 360.0  ## Horizontal field of view in degrees
@export var vertical_fov: float = 30.0  ## Vertical field of view in degrees
@export var horizontal_resolution: float = 1.0  ## Horizontal angular resolution in degrees
@export var vertical_resolution: float = 1.0  ## Vertical angular resolution in degrees

@export_group("Performance")
@export var max_points_per_scan: int = 1000  ## Maximum points per scan
@export var collision_mask: int = 1  ## Physics collision mask for raycasting

@export_group("Noise Model")
@export var range_noise_std_dev: float = 0.01  ## Range measurement noise in meters
@export var angular_noise_std_dev: float = 0.1  ## Angular noise in degrees
@export var dropout_probability: float = 0.0  ## Probability of missing detection (0.0 to 1.0)

enum LidarMode {
	SINGLE_BEAM,  ## Single distance measurement
	HORIZONTAL_SCAN,  ## 2D horizontal scan
	FULL_3D  ## 3D point cloud
}

# Measurement data
var distance: float = 0.0  ## Single beam distance
var scan_points: Array[Vector3] = []  ## Point cloud in local frame
var scan_ranges: Array[float] = []  ## Distances for each scan point
var hit_normals: Array[Vector3] = []  ## Surface normals at hit points

# Internal
var space_state: PhysicsDirectSpaceState3D
var scan_progress: float = 0.0

func _ready():
	super._ready()
	beam_direction = beam_direction.normalized()
	mass = 1.0 + (0.5 if lidar_mode == LidarMode.FULL_3D else 0.0)
	power_consumption = 5.0 + (10.0 if lidar_mode == LidarMode.FULL_3D else 0.0)
	
	# Get physics space
	space_state = get_world_3d().direct_space_state

func _update_measurement():
	if not space_state:
		space_state = get_world_3d().direct_space_state
		if not space_state:
			is_valid = false
			return
	
	match lidar_mode:
		LidarMode.SINGLE_BEAM:
			_measure_single_beam()
		LidarMode.HORIZONTAL_SCAN:
			_measure_horizontal_scan()
		LidarMode.FULL_3D:
			_measure_3d_scan()
	
	measurement = {
		"distance": distance,
		"scan_points": scan_points,
		"scan_ranges": scan_ranges,
		"hit_normals": hit_normals
	}

## Measures distance along a single beam.
func _measure_single_beam():
	var ray_result = _cast_ray(beam_direction)
	
	if ray_result:
		distance = ray_result.distance
		if add_noise:
			distance = add_gaussian_noise_custom(distance, range_noise_std_dev)
		distance = clamp(distance, min_range, max_range)
	else:
		distance = max_range

## Performs a 2D horizontal scan.
func _measure_horizontal_scan():
	scan_points.clear()
	scan_ranges.clear()
	hit_normals.clear()
	
	var start_angle = -horizontal_fov / 2.0
	var num_rays = int(horizontal_fov / horizontal_resolution)
	num_rays = min(num_rays, max_points_per_scan)
	
	for i in range(num_rays):
		var angle = start_angle + i * horizontal_resolution
		var direction = _rotate_vector_around_y(beam_direction, deg_to_rad(angle))
		
		var ray_result = _cast_ray(direction)
		
		if ray_result and randf() > dropout_probability:
			var range_val = ray_result.distance
			if add_noise:
				range_val = add_gaussian_noise_custom(range_val, range_noise_std_dev)
			range_val = clamp(range_val, min_range, max_range)
			
			var point = direction * range_val
			scan_points.append(point)
			scan_ranges.append(range_val)
			hit_normals.append(ray_result.normal)

## Performs a full 3D scan.
func _measure_3d_scan():
	scan_points.clear()
	scan_ranges.clear()
	hit_normals.clear()
	
	var h_start = -horizontal_fov / 2.0
	var v_start = -vertical_fov / 2.0
	
	var h_rays = int(horizontal_fov / horizontal_resolution)
	var v_rays = int(vertical_fov / vertical_resolution)
	
	var total_rays = h_rays * v_rays
	if total_rays > max_points_per_scan:
		# Reduce resolution to stay within limit
		var scale = sqrt(float(max_points_per_scan) / total_rays)
		h_rays = int(h_rays * scale)
		v_rays = int(v_rays * scale)
	
	for v in range(v_rays):
		var v_angle = v_start + v * vertical_resolution
		
		for h in range(h_rays):
			var h_angle = h_start + h * horizontal_resolution
			
			var direction = _get_scan_direction(h_angle, v_angle)
			var ray_result = _cast_ray(direction)
			
			if ray_result and randf() > dropout_probability:
				var range_val = ray_result.distance
				if add_noise:
					range_val = add_gaussian_noise_custom(range_val, range_noise_std_dev)
				range_val = clamp(range_val, min_range, max_range)
				
				var point = direction * range_val
				scan_points.append(point)
				scan_ranges.append(range_val)
				hit_normals.append(ray_result.normal)

## Casts a ray in the given local direction.
func _cast_ray(local_direction: Vector3) -> Dictionary:
	var global_origin = global_position
	var global_direction = global_transform.basis * local_direction.normalized()
	var ray_end = global_origin + global_direction * max_range
	
	var query = PhysicsRayQueryParameters3D.create(global_origin, ray_end)
	query.collision_mask = collision_mask
	query.exclude = [get_parent()]  # Don't hit own vehicle
	
	var result = space_state.intersect_ray(query)
	
	if result:
		var hit_point = result.position
		var distance_val = global_origin.distance_to(hit_point)
		var normal = result.normal
		
		return {
			"distance": distance_val,
			"position": hit_point,
			"normal": normal,
			"collider": result.collider
		}
	
	return {}

## Rotates a vector around the Y axis.
func _rotate_vector_around_y(vec: Vector3, angle_rad: float) -> Vector3:
	var cos_a = cos(angle_rad)
	var sin_a = sin(angle_rad)
	return Vector3(
		vec.x * cos_a - vec.z * sin_a,
		vec.y,
		vec.x * sin_a + vec.z * cos_a
	)

## Gets scan direction for given horizontal and vertical angles.
func _get_scan_direction(h_angle_deg: float, v_angle_deg: float) -> Vector3:
	var h_rad = deg_to_rad(h_angle_deg)
	var v_rad = deg_to_rad(v_angle_deg)
	
	# Start with beam direction, rotate around Y (horizontal), then X (vertical)
	var dir = beam_direction
	dir = _rotate_vector_around_y(dir, h_rad)
	dir = _rotate_vector_around_x(dir, v_rad)
	
	return dir.normalized()

## Rotates a vector around the X axis.
func _rotate_vector_around_x(vec: Vector3, angle_rad: float) -> Vector3:
	var cos_a = cos(angle_rad)
	var sin_a = sin(angle_rad)
	return Vector3(
		vec.x,
		vec.y * cos_a - vec.z * sin_a,
		vec.y * sin_a + vec.z * cos_a
	)

## Returns the point cloud in global coordinates.
func get_global_point_cloud() -> Array[Vector3]:
	var global_points: Array[Vector3] = []
	for local_point in scan_points:
		global_points.append(global_transform * local_point)
	return global_points

## Returns the closest detected point.
func get_closest_point() -> Vector3:
	if scan_ranges.is_empty():
		return Vector3.ZERO
	
	var min_idx = 0
	var min_range = scan_ranges[0]
	
	for i in range(1, scan_ranges.size()):
		if scan_ranges[i] < min_range:
			min_range = scan_ranges[i]
			min_idx = i
	
	return scan_points[min_idx]

## Returns the number of detected points.
func get_point_count() -> int:
	return scan_points.size()

func _update_telemetry():
	super._update_telemetry()
	Telemetry["distance"] = distance
	Telemetry["point_count"] = scan_points.size()
	Telemetry["mode"] = LidarMode.keys()[lidar_mode]
