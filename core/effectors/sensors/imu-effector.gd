class_name LCIMUEffector
extends LCSensorEffector

## Inertial Measurement Unit (IMU) sensor effector.
##
## Measures linear acceleration and angular velocity.
## Includes gyroscope and accelerometer with realistic noise and bias.

@export_group("IMU Configuration")
@export var measure_acceleration: bool = true  ## Enable accelerometer
@export var measure_angular_velocity: bool = true  ## Enable gyroscope
@export var measure_in_body_frame: bool = true  ## Measure in body frame (vs global)

@export_group("Accelerometer")
@export var accel_noise_std_dev: Vector3 = Vector3(0.01, 0.01, 0.01)  ## m/s² noise
@export var accel_bias: Vector3 = Vector3.ZERO  ## m/s² constant bias
@export var accel_range: float = 100.0  ## Maximum measurable acceleration in m/s²

@export_group("Gyroscope")
@export var gyro_noise_std_dev: Vector3 = Vector3(0.001, 0.001, 0.001)  ## rad/s noise
@export var gyro_bias: Vector3 = Vector3.ZERO  ## rad/s constant bias
@export var gyro_range: float = 10.0  ## Maximum measurable angular velocity in rad/s

@export_group("Bias Drift")
@export var enable_bias_drift: bool = false  ## Enable random walk bias drift
@export var accel_bias_drift_rate: float = 0.0001  ## m/s² per second
@export var gyro_bias_drift_rate: float = 0.00001  ## rad/s per second

# Measurement data
var linear_acceleration: Vector3 = Vector3.ZERO  ## Measured linear acceleration
var angular_velocity: Vector3 = Vector3.ZERO  ## Measured angular velocity

# Internal state
var previous_velocity: Vector3 = Vector3.ZERO
var current_accel_bias: Vector3 = Vector3.ZERO
var current_gyro_bias: Vector3 = Vector3.ZERO

# Reference to parent vehicle
var vehicle: RigidBody3D

func _ready():
	super._ready()
	mass = 0.2  # Typical IMU mass
	power_consumption = 1.0  # Typical IMU power
	
	current_accel_bias = accel_bias
	current_gyro_bias = gyro_bias
	
	# Find parent vehicle
	var parent = get_parent()
	while parent:
		if parent is RigidBody3D:
			vehicle = parent
			break
		parent = parent.get_parent()

func _update_measurement():
	if not vehicle:
		is_valid = false
		return
	
	# Update bias drift
	if enable_bias_drift:
		_update_bias_drift()
	
	# Measure angular velocity (gyroscope)
	if measure_angular_velocity:
		angular_velocity = _measure_gyroscope()
	
	# Measure linear acceleration (accelerometer)
	if measure_acceleration:
		linear_acceleration = _measure_accelerometer()
	
	measurement = {
		"linear_acceleration": linear_acceleration,
		"angular_velocity": angular_velocity
	}

## Measures angular velocity using gyroscope.
func _measure_gyroscope() -> Vector3:
	var true_angular_vel = vehicle.angular_velocity
	
	# Convert to body frame if needed
	if measure_in_body_frame:
		true_angular_vel = vehicle.global_transform.basis.inverse() * true_angular_vel
	
	# Add noise
	var measured = true_angular_vel
	if add_noise:
		measured = add_gaussian_noise_vec3(measured, gyro_noise_std_dev)
	
	# Add bias
	if add_bias:
		measured += current_gyro_bias
	
	# Clamp to sensor range
	measured.x = clamp(measured.x, -gyro_range, gyro_range)
	measured.y = clamp(measured.y, -gyro_range, gyro_range)
	measured.z = clamp(measured.z, -gyro_range, gyro_range)
	
	return measured

## Measures linear acceleration using accelerometer.
func _measure_accelerometer() -> Vector3:
	var true_accel = vehicle.linear_velocity - previous_velocity
	true_accel /= (1.0 / update_rate) if update_rate > 0 else 0.01
	previous_velocity = vehicle.linear_velocity
	
	# Add gravity (accelerometer measures specific force, not coordinate acceleration)
	var gravity = Vector3(0, -9.81, 0)  # Assuming Earth gravity
	true_accel -= gravity
	
	# Convert to body frame if needed
	if measure_in_body_frame:
		true_accel = vehicle.global_transform.basis.inverse() * true_accel
	
	# Add noise
	var measured = true_accel
	if add_noise:
		measured = add_gaussian_noise_vec3(measured, accel_noise_std_dev)
	
	# Add bias
	if add_bias:
		measured += current_accel_bias
	
	# Clamp to sensor range
	measured.x = clamp(measured.x, -accel_range, accel_range)
	measured.y = clamp(measured.y, -accel_range, accel_range)
	measured.z = clamp(measured.z, -accel_range, accel_range)
	
	return measured

## Updates bias drift (random walk).
func _update_bias_drift():
	var dt = 1.0 / update_rate if update_rate > 0 else 0.01
	
	# Accelerometer bias drift
	current_accel_bias.x += randf_range(-accel_bias_drift_rate, accel_bias_drift_rate) * dt
	current_accel_bias.y += randf_range(-accel_bias_drift_rate, accel_bias_drift_rate) * dt
	current_accel_bias.z += randf_range(-accel_bias_drift_rate, accel_bias_drift_rate) * dt
	
	# Gyroscope bias drift
	current_gyro_bias.x += randf_range(-gyro_bias_drift_rate, gyro_bias_drift_rate) * dt
	current_gyro_bias.y += randf_range(-gyro_bias_drift_rate, gyro_bias_drift_rate) * dt
	current_gyro_bias.z += randf_range(-gyro_bias_drift_rate, gyro_bias_drift_rate) * dt

## Returns the measured linear acceleration.
func get_linear_acceleration() -> Vector3:
	return linear_acceleration if is_valid else Vector3.ZERO

## Returns the measured angular velocity.
func get_angular_velocity() -> Vector3:
	return angular_velocity if is_valid else Vector3.ZERO

## Resets bias to initial values.
func reset_bias():
	current_accel_bias = accel_bias
	current_gyro_bias = gyro_bias

func _update_telemetry():
	super._update_telemetry()
	Telemetry["linear_acceleration"] = linear_acceleration
	Telemetry["angular_velocity"] = angular_velocity
	Telemetry["accel_bias"] = current_accel_bias
	Telemetry["gyro_bias"] = current_gyro_bias
