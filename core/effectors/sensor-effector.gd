class_name LCSensorEffector
extends LCStateEffector

## Base class for sensor effectors.
##
## Sensors provide measurements of the environment or vehicle state.
## They can include noise, bias, and other realistic effects.

@export_group("Sensor Properties")
@export var update_rate: float = 10.0  ## Sensor update rate in Hz
@export var is_enabled: bool = true  ## Is sensor active?
@export var add_noise: bool = true  ## Add measurement noise?
@export var add_bias: bool = false  ## Add measurement bias?

@export_group("Noise Model")
@export var noise_std_dev: float = 0.01  ## Standard deviation of noise
@export var bias_value: float = 0.0  ## Constant bias value

# Internal state
var last_update_time: float = 0.0
var measurement_count: int = 0
var is_valid: bool = false  ## Is current measurement valid?

# Measurement data (override in subclasses)
var measurement: Variant = null

func _ready():
	super._ready()
	mass = 0.5  # Typical sensor mass
	power_consumption = 2.0  # Typical sensor power
	_initialize_telemetry()

func _physics_process(delta):
	if not is_enabled:
		is_valid = false
		return
	
	# Update at specified rate
	var update_interval = 1.0 / update_rate if update_rate > 0 else 0.0
	last_update_time += delta
	
	if last_update_time >= update_interval:
		_update_measurement()
		last_update_time = 0.0
		measurement_count += 1
		is_valid = true
	
	_update_telemetry()

## Override this to implement sensor-specific measurement logic.
func _update_measurement():
	pass

## Adds Gaussian noise to a value.
func add_gaussian_noise(value: float) -> float:
	if not add_noise:
		return value
	
	# Box-Muller transform for Gaussian noise
	var u1 = randf()
	var u2 = randf()
	var noise = sqrt(-2.0 * log(u1)) * cos(2.0 * PI * u2) * noise_std_dev
	return value + noise

## Adds Gaussian noise to a Vector3.
func add_gaussian_noise_vec3(value: Vector3, std_dev: Vector3 = Vector3.ONE) -> Vector3:
	if not add_noise:
		return value
	
	return Vector3(
		add_gaussian_noise_custom(value.x, std_dev.x),
		add_gaussian_noise_custom(value.y, std_dev.y),
		add_gaussian_noise_custom(value.z, std_dev.z)
	)

## Adds Gaussian noise with custom standard deviation.
func add_gaussian_noise_custom(value: float, std_dev: float) -> float:
	if not add_noise:
		return value
	
	var u1 = randf()
	var u2 = randf()
	var noise = sqrt(-2.0 * log(u1)) * cos(2.0 * PI * u2) * std_dev
	return value + noise

## Adds bias to a value.
func add_measurement_bias(value: float) -> float:
	if not add_bias:
		return value
	return value + bias_value

## Enables the sensor.
func enable():
	is_enabled = true

## Disables the sensor.
func disable():
	is_enabled = false
	is_valid = false

## Returns the current measurement.
func get_measurement() -> Variant:
	return measurement if is_valid else null

## Returns true if measurement is valid.
func is_measurement_valid() -> bool:
	return is_valid and is_enabled

func _initialize_telemetry():
	Telemetry = {
		"is_enabled": is_enabled,
		"is_valid": is_valid,
		"update_rate": update_rate,
		"measurement_count": measurement_count,
	}

func _update_telemetry():
	Telemetry["is_enabled"] = is_enabled
	Telemetry["is_valid"] = is_valid
	Telemetry["measurement_count"] = measurement_count
