class_name LCReactionWheelEffector
extends LCStateEffector

## Reaction wheel state effector for momentum storage and attitude control.
##
## Stores angular momentum and applies torque to the spacecraft.
## Includes saturation limits, friction, and power consumption.

@export_group("Reaction Wheel Properties")
@export var max_torque: float = 0.1  ## Maximum torque in N·m
@export var max_momentum: float = 10.0  ## Maximum momentum storage in N·m·s
@export var wheel_inertia: float = 0.01  ## Wheel moment of inertia in kg·m²
@export var spin_axis: Vector3 = Vector3(1, 0, 0)  ## Local spin axis (normalized)

@export_group("Performance")
@export var friction_coefficient: float = 0.001  ## Friction torque coefficient
@export var motor_efficiency: float = 0.9  ## Motor efficiency (0.0 to 1.0)
@export var static_power: float = 5.0  ## Static power draw in Watts
@export var torque_power_ratio: float = 50.0  ## Watts per N·m of torque

@export_group("Limits")
@export var enable_saturation: bool = true  ## Enable momentum saturation
@export var enable_friction: bool = true  ## Enable friction losses

# Control inputs
var torque_command: float = 0.0  ## Commanded torque in N·m (-max_torque to +max_torque)

# Internal state
var wheel_speed: float = 0.0  ## Wheel speed in rad/s
var stored_momentum: float = 0.0  ## Stored angular momentum in N·m·s
var is_saturated: bool = false
var total_momentum_dumped: float = 0.0  ## Total momentum dumped via external torques

func _ready():
	super._ready()
	spin_axis = spin_axis.normalized()
	mass = 2.0  # Typical RW mass
	_initialize_telemetry()

func _physics_process(delta):
	_update_wheel_dynamics(delta)
	_update_power_consumption()

func _process(delta):
	_update_telemetry()

## Sets the commanded torque (-1.0 to 1.0, normalized).
func set_torque_normalized(level: float):
	torque_command = clamp(level, -1.0, 1.0) * max_torque

## Sets the commanded torque in N·m.
func set_torque(torque_nm: float):
	torque_command = clamp(torque_nm, -max_torque, max_torque)

## Updates wheel dynamics and momentum.
func _update_wheel_dynamics(delta: float):
	# Apply commanded torque
	var net_torque = torque_command
	
	# Apply friction
	if enable_friction:
		var friction_torque = -sign(wheel_speed) * friction_coefficient * abs(wheel_speed)
		net_torque += friction_torque
	
	# Check saturation
	if enable_saturation:
		if abs(stored_momentum) >= max_momentum:
			is_saturated = true
			# Prevent further momentum increase in saturation direction
			if sign(net_torque) == sign(stored_momentum):
				net_torque = 0.0
		else:
			is_saturated = false
	
	# Update wheel speed and momentum
	if wheel_inertia > 0:
		var wheel_accel = net_torque / wheel_inertia
		wheel_speed += wheel_accel * delta
		stored_momentum = wheel_speed * wheel_inertia
		
		# Clamp momentum
		if enable_saturation:
			stored_momentum = clamp(stored_momentum, -max_momentum, max_momentum)
			wheel_speed = stored_momentum / wheel_inertia

## Returns the torque applied to the spacecraft (reaction torque).
## This should be called by the vehicle to apply the RW torque.
func get_reaction_torque() -> Vector3:
	# Reaction torque is opposite to wheel acceleration
	var wheel_torque = -torque_command
	return local_to_global_torque(spin_axis * wheel_torque)

## Dumps momentum by applying external torque (e.g., thrusters, magnetic torquers).
## Returns actual momentum dumped.
func dump_momentum(external_torque: float, delta: float) -> float:
	var momentum_change = external_torque * delta
	var actual_dump = clamp(momentum_change, -abs(stored_momentum), abs(stored_momentum))
	
	stored_momentum -= actual_dump
	if wheel_inertia > 0:
		wheel_speed = stored_momentum / wheel_inertia
	
	total_momentum_dumped += abs(actual_dump)
	return actual_dump

## Returns true if wheel is saturated.
func is_wheel_saturated() -> bool:
	return is_saturated

## Returns momentum margin (0.0 = saturated, 1.0 = empty).
func get_momentum_margin() -> float:
	return 1.0 - (abs(stored_momentum) / max_momentum) if max_momentum > 0 else 0.0

## Updates power consumption based on torque and speed.
func _update_power_consumption():
	# Power = static + torque-dependent + speed-dependent
	var torque_power = abs(torque_command) * torque_power_ratio / motor_efficiency
	var speed_power = abs(wheel_speed) * 0.1  # Small speed-dependent term
	power_consumption = static_power + torque_power + speed_power

## Helper to convert local torque to global.
func local_to_global_torque(local_torque: Vector3) -> Vector3:
	return global_transform.basis * local_torque

func _initialize_telemetry():
	Telemetry = {
		"torque_command": torque_command,
		"wheel_speed": wheel_speed,
		"stored_momentum": stored_momentum,
		"is_saturated": is_saturated,
		"momentum_margin": get_momentum_margin(),
		"power_consumption": power_consumption,
	}

func _update_telemetry():
	Telemetry["torque_command"] = torque_command
	Telemetry["wheel_speed"] = wheel_speed
	Telemetry["stored_momentum"] = stored_momentum
	Telemetry["is_saturated"] = is_saturated
	Telemetry["momentum_margin"] = get_momentum_margin()
	Telemetry["power_consumption"] = power_consumption

# Command interface
func cmd_set_torque(args: Array):
	if args.size() >= 1:
		set_torque(args[0])

func cmd_dump_momentum(args: Array):
	if args.size() >= 2:
		dump_momentum(args[0], args[1])
