class_name LCGPSEffector
extends LCSensorEffector

## GPS (Global Positioning System) sensor effector.
##
## Provides position and velocity measurements with realistic
## accuracy degradation and satellite availability modeling.

@export_group("GPS Configuration")
@export var gps_mode: GPSMode = GPSMode.STANDARD
@export var measure_velocity: bool = true  ## Enable velocity measurement
@export var use_local_coordinates: bool = false  ## Use local coords instead of lat/lon

@export_group("Accuracy")
@export var position_noise_std_dev: Vector3 = Vector3(5.0, 5.0, 10.0)  ## meters (horizontal, horizontal, vertical)
@export var velocity_noise_std_dev: Vector3 = Vector3(0.1, 0.1, 0.2)  ## m/s
@export var altitude_bias: float = 0.0  ## Constant altitude bias in meters

@export_group("Signal Quality")
@export var min_satellites: int = 4  ## Minimum satellites for fix
@export var satellite_availability: float = 1.0  ## Probability of having enough satellites (0.0 to 1.0)
@export var multipath_error: float = 2.0  ## Additional error from multipath in meters

enum GPSMode {
	STANDARD,  ## Standard GPS (~5-10m accuracy)
	DGPS,  ## Differential GPS (~1-3m accuracy)
	RTK  ## Real-Time Kinematic (~0.01-0.1m accuracy)
}

# Measurement data
var measured_position: Vector3 = Vector3.ZERO  ## Measured position (local or global)
var velocity: Vector3 = Vector3.ZERO  ## Measured velocity
var latitude: float = 0.0  ## Latitude in degrees
var longitude: float = 0.0  ## Longitude in degrees
var altitude: float = 0.0  ## Altitude in meters
var has_fix: bool = false  ## GPS fix available
var satellite_count: int = 0  ## Number of visible satellites
var hdop: float = 1.0  ## Horizontal Dilution of Precision

# Internal
var vehicle: Node3D
var reference_position: Vector3 = Vector3.ZERO  ## Reference for lat/lon conversion

func _ready():
	super._ready()
	mass = 0.1  # Typical GPS receiver mass
	power_consumption = 0.5  # Typical GPS power
	
	# Set noise based on GPS mode
	_configure_for_mode()
	
	# Find parent vehicle
	vehicle = get_parent()
	while vehicle and not vehicle is Node3D:
		vehicle = vehicle.get_parent()

func _configure_for_mode():
	match gps_mode:
		GPSMode.STANDARD:
			position_noise_std_dev = Vector3(5.0, 5.0, 10.0)
			update_rate = 1.0  # 1 Hz
		GPSMode.DGPS:
			position_noise_std_dev = Vector3(1.0, 1.0, 2.0)
			update_rate = 5.0  # 5 Hz
		GPSMode.RTK:
			position_noise_std_dev = Vector3(0.02, 0.02, 0.05)
			update_rate = 10.0  # 10 Hz

func _update_measurement():
	if not vehicle:
		is_valid = false
		return
	
	# Simulate satellite availability
	satellite_count = _simulate_satellite_count()
	has_fix = satellite_count >= min_satellites and randf() < satellite_availability
	
	if not has_fix:
		is_valid = false
		return
	
	# Calculate HDOP (simplified model)
	hdop = 1.0 + randf() * 2.0
	
	# Measure position
	var true_position = vehicle.global_position
	measured_position = _measure_position(true_position)
	
	# Convert to lat/lon if needed
	if not use_local_coordinates:
		_convert_to_latlon(measured_position)
	
	# Measure velocity
	if measure_velocity:
		var true_velocity = vehicle.linear_velocity if vehicle is RigidBody3D else Vector3.ZERO
		velocity = _measure_velocity(true_velocity)
	
	measurement = {
		"position": measured_position,
		"velocity": velocity,
		"latitude": latitude,
		"longitude": longitude,
		"altitude": altitude,
		"has_fix": has_fix,
		"satellite_count": satellite_count,
		"hdop": hdop
	}

## Measures position with GPS noise.
func _measure_position(true_pos: Vector3) -> Vector3:
	var measured = true_pos
	
	# Add GPS noise (different for horizontal and vertical)
	if add_noise:
		measured.x = add_gaussian_noise_custom(measured.x, position_noise_std_dev.x * hdop)
		measured.y = add_gaussian_noise_custom(measured.y, position_noise_std_dev.z * hdop)  # Vertical
		measured.z = add_gaussian_noise_custom(measured.z, position_noise_std_dev.y * hdop)
		
		# Add multipath error
		measured.x += randf_range(-multipath_error, multipath_error)
		measured.z += randf_range(-multipath_error, multipath_error)
	
	# Add altitude bias
	if add_bias:
		measured.y += altitude_bias
	
	return measured

## Measures velocity with GPS noise.
func _measure_velocity(true_vel: Vector3) -> Vector3:
	var measured = true_vel
	
	if add_noise:
		measured = add_gaussian_noise_vec3(measured, velocity_noise_std_dev * hdop)
	
	return measured

## Simulates satellite count based on availability.
func _simulate_satellite_count() -> int:
	# Typical GPS sees 6-12 satellites
	var base_count = randi_range(6, 12)
	
	# Reduce based on availability
	if satellite_availability < 1.0:
		base_count = int(base_count * satellite_availability)
	
	return max(0, base_count)

## Converts local position to latitude/longitude/altitude.
## This is a simplified conversion - real GPS would use WGS84 ellipsoid.
func _convert_to_latlon(local_pos: Vector3):
	# Simple conversion: 1 degree latitude ≈ 111 km
	# 1 degree longitude ≈ 111 km * cos(latitude)
	
	var meters_per_degree_lat = 111000.0
	var meters_per_degree_lon = 111000.0  # Simplified, should vary with latitude
	
	latitude = reference_position.x + (local_pos.z / meters_per_degree_lat)
	longitude = reference_position.z + (local_pos.x / meters_per_degree_lon)
	altitude = local_pos.y

## Sets the reference position for lat/lon conversion.
func set_reference_latlon(lat: float, lon: float, alt: float = 0.0):
	reference_position = Vector3(lat, alt, lon)

## Returns true if GPS has a valid fix.
func has_valid_fix() -> bool:
	return has_fix and is_valid

## Returns the measured position.
func get_measured_position() -> Vector3:
	return measured_position if is_valid else Vector3.ZERO

## Returns the measured velocity.
func get_velocity() -> Vector3:
	return velocity if is_valid else Vector3.ZERO

## Returns position accuracy estimate in meters.
func get_position_accuracy() -> float:
	if not has_fix:
		return 999.9
	
	# Simplified accuracy estimate
	var base_accuracy = position_noise_std_dev.x
	return base_accuracy * hdop

func _update_telemetry():
	super._update_telemetry()
	Telemetry["position"] = measured_position
	Telemetry["velocity"] = velocity
	Telemetry["has_fix"] = has_fix
	Telemetry["satellite_count"] = satellite_count
	Telemetry["hdop"] = hdop
	Telemetry["accuracy"] = get_position_accuracy()
	
	if not use_local_coordinates:
		Telemetry["latitude"] = latitude
		Telemetry["longitude"] = longitude
		Telemetry["altitude"] = altitude
