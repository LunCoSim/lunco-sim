class_name LCWheelEffector
extends VehicleWheel3D

## Wheel component that acts as both a state effector (mass) and dynamic effector (torque).
## Extends VehicleWheel3D directly to work with Godot's vehicle physics.

# --- LCComponent / LCStateEffector Interface ---
@export var mass: float = 10.0
@export var power_consumption: float = 0.0
@export var power_production: float = 0.0
@export var attachment_nodes: Array[Node3D] = []

@export_category("XTCE")
@export var Telemetry = {}
@export var Parameters = {}
@export var Commands = {}

func get_mass_contribution() -> float:
	return mass

func get_inertia_contribution() -> Vector3:
	return Vector3.ZERO # Point mass approximation

func get_center_of_mass_offset() -> Vector3:
	return position # Relative to vehicle body

func get_power_consumption() -> float:
	return power_consumption

func get_power_production() -> float:
	return power_production

# Motor and brake inputs
@export var motor_torque_request: float = 0.0
@export var brake_force_request: float = 0.0

# Wheel parameters
@export var wheel_radius_config: float = 0.3:
	set(value):
		wheel_radius_config = value
		wheel_radius = value # Update built-in property

@export var max_motor_torque: float = 1000.0
@export var max_brake_force: float = 800.0

func _ready():
	# Apply properties
	wheel_radius = wheel_radius_config
	
	# Initialize telemetry
	Telemetry["current_speed"] = 0.0
	Telemetry["motor_torque"] = 0.0
	Telemetry["brake_force"] = 0.0
	Telemetry["rpm"] = 0.0

func _physics_process(delta):
	_update_telemetry()

## Implements dynamic effector interface (duck typed)
func compute_force_torque(delta: float) -> Dictionary:
	# VehicleWheel3D handles physics internally via engine_force/brake
	# We just report what we WOULD do for consistency
	return {
		"force": Vector3.ZERO,
		"torque": Vector3(0, motor_torque_request, 0),
		"position": global_position
	}

## Sets motor torque (called by controller).
func set_motor_torque(torque: float):
	motor_torque_request = clamp(torque, -max_motor_torque, max_motor_torque)
	Commands["motor_torque"] = motor_torque_request

## Sets brake force (called by controller).
func set_brake_force(force: float):
	brake_force_request = clamp(force, 0.0, max_brake_force)
	Commands["brake"] = brake_force_request

## Gets current wheel speed in m/s.
func get_wheel_speed() -> float:
	# RPM to m/s: (RPM * 2π * radius) / 60
	var rpm = get_rpm()
	return (rpm * TAU * wheel_radius) / 60.0

## Gets current wheel RPM.
func get_wheel_rpm() -> float:
	return get_rpm()

## Updates telemetry
func _update_telemetry():
	Telemetry["current_speed"] = get_wheel_speed()
	Telemetry["motor_torque"] = motor_torque_request
	Telemetry["brake_force"] = brake_force_request
	Telemetry["rpm"] = get_wheel_rpm()
