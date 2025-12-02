class_name LCThrusterEffector
extends LCDynamicEffector

## Thruster dynamic effector that applies thrust forces and depletes fuel.
##
## Can be linked to a fuel tank for mass depletion.
## Supports thrust vectoring and on/off pulsing.

@export_group("Thruster Properties")
@export var max_thrust: float = 100.0  ## Maximum thrust in Newtons
@export var specific_impulse: float = 300.0  ## Isp in seconds
@export var min_on_time: float = 0.02  ## Minimum firing pulse in seconds
@export var thrust_direction: Vector3 = Vector3(0, 0, 1)  ## Local thrust direction (normalized)

@export_group("Propellant Connection")
@export var fuel_tank_path: NodePath
var fuel_tank = null  ## Can be LCFuelTankEffector or LCResourceTankEffector
@export var oxidizer_tank_path: NodePath
var oxidizer_tank = null  ## LCResourceTankEffector for oxygen
@export var mixture_ratio: float = 3.6  ## Oxidizer:Fuel mass ratio (3.6:1 for Starship Raptor)
@export var fuel_flow_rate: float = 0.0  ## kg/s at max thrust (auto-calculated if 0)

@export_group("Thrust Vectoring")
@export var can_vector: bool = false  ## Can this thruster gimbal?
@export var max_gimbal_angle: float = 5.0  ## Maximum gimbal angle in degrees
@export var gimbal_rate: float = 10.0  ## Gimbal rate in degrees/second

@export_group("Performance")
@export var thrust_ramp_time: float = 0.1  ## Time to reach full thrust in seconds
@export var efficiency: float = 1.0  ## Thrust efficiency (0.0 to 1.0)

# Control inputs
var thrust_command: float = 0.0  ## Commanded thrust level (0.0 to 1.0)
var gimbal_command: Vector2 = Vector2.ZERO  ## Gimbal command in degrees (pitch, yaw)

# Internal state
var current_thrust: float = 0.0  ## Current actual thrust
var current_gimbal: Vector2 = Vector2.ZERO  ## Current gimbal angles
var is_firing: bool = false
var firing_time: float = 0.0
var total_impulse: float = 0.0  ## Total impulse delivered in N·s

# Physics constants
const G0: float = 9.80665  ## Standard gravity in m/s²

func _ready():
	super._ready()
	thrust_direction = thrust_direction.normalized()
	
	# Connect to fuel tank (can be LCFuelTankEffector or LCResourceTankEffector)
	if not fuel_tank_path.is_empty():
		var node = get_node_or_null(fuel_tank_path)
		if node is LCFuelTankEffector or node is LCResourceTankEffector:
			fuel_tank = node
			var amount = 0.0
			if node is LCFuelTankEffector:
				amount = node.fuel_mass
			else:
				amount = node.get_amount()
			print("✓ LCThrusterEffector: Connected to fuel tank ", node.name, " with ", amount, " kg")
		else:
			print("✗ LCThrusterEffector: Invalid fuel tank path or type: ", fuel_tank_path)
	else:
		print("✗ LCThrusterEffector: No fuel tank path set")
	
	# Connect to oxidizer tank
	if not oxidizer_tank_path.is_empty():
		var node = get_node_or_null(oxidizer_tank_path)
		if node is LCResourceTankEffector:
			oxidizer_tank = node
			print("✓ LCThrusterEffector: Connected to oxidizer tank ", node.name, " with ", node.get_amount(), " kg")
		else:
			print("✗ LCThrusterEffector: Invalid oxidizer tank path or type: ", oxidizer_tank_path)
	else:
		print("✗ LCThrusterEffector: No oxidizer tank path set")
	
	# Auto-calculate fuel flow rate if not set
	# Total propellant flow = fuel + oxidizer
	# For mixture ratio R:1, fuel fraction = 1/(R+1), oxidizer fraction = R/(R+1)
	if fuel_flow_rate <= 0.0 and specific_impulse > 0.0:
		var total_flow = max_thrust / (specific_impulse * G0)
		fuel_flow_rate = total_flow / (mixture_ratio + 1.0)  # Fuel portion only
		print("✓ LCThrusterEffector: Calculated fuel flow rate: ", fuel_flow_rate, " kg/s")
	
	# Set power consumption (rough estimate: 10W per 100N)
	power_consumption = max_thrust * 0.1
	
	_initialize_telemetry()

func _physics_process(delta):
	_update_thrust(delta)
	_update_gimbal(delta)
	_update_telemetry()
	# Only show plume when actually firing (after fuel check)
	# visible is set in compute_force_torque based on can_fire

## Sets the thrust command (0.0 to 1.0).
func set_thrust(level: float):
	thrust_command = clamp(level, 0.0, 1.0)

## Sets the gimbal command in degrees.
func set_gimbal(pitch_deg: float, yaw_deg: float):
	if can_vector:
		gimbal_command.x = clamp(pitch_deg, -max_gimbal_angle, max_gimbal_angle)
		gimbal_command.y = clamp(yaw_deg, -max_gimbal_angle, max_gimbal_angle)

## Fires a thrust pulse for the given duration.
func fire_pulse(duration: float, level: float = 1.0):
	if duration >= min_on_time:
		set_thrust(level)
		# Note: Caller should handle timing or use a timer

## Updates thrust level with ramping.
func _update_thrust(delta: float):
	var target_thrust = thrust_command * max_thrust * efficiency
	
	# Ramp thrust
	if thrust_ramp_time > 0:
		var ramp_rate = max_thrust / thrust_ramp_time
		if current_thrust < target_thrust:
			current_thrust = min(current_thrust + ramp_rate * delta, target_thrust)
		else:
			current_thrust = max(current_thrust - ramp_rate * delta, target_thrust)
	else:
		current_thrust = target_thrust
	
	is_firing = current_thrust > 0.01
	
	if is_firing:
		firing_time += delta
		total_impulse += current_thrust * delta

## Updates gimbal angles.
func _update_gimbal(delta: float):
	if can_vector:
		# Ramp gimbal angles
		var gimbal_delta = gimbal_rate * delta
		current_gimbal.x = move_toward(current_gimbal.x, gimbal_command.x, gimbal_delta)
		current_gimbal.y = move_toward(current_gimbal.y, gimbal_command.y, gimbal_delta)

## Computes force and torque from thruster.
func compute_force_torque(delta: float) -> Dictionary:
	# Update visibility based on firing state
	visible = is_firing
	
	if not is_firing:
		return {}
	
	# Deplete propellants if connected to tanks
	var thrust_fraction = current_thrust / max_thrust if max_thrust > 0 else 0.0
	var can_fire = true
	
	# Calculate propellant needs based on mixture ratio
	var fuel_needed = fuel_flow_rate * thrust_fraction * delta
	var oxidizer_needed = fuel_needed * mixture_ratio
	
	# Deplete fuel tank
	if fuel_tank:
		var fuel_available = 0.0
		if fuel_tank is LCFuelTankEffector:
			fuel_available = fuel_tank.deplete_fuel(fuel_needed)
		elif fuel_tank is LCResourceTankEffector:
			fuel_available = fuel_tank.remove_resource(fuel_needed)
		
		if fuel_available < fuel_needed * 0.99:  # Allow 1% tolerance
			can_fire = false
			if fuel_tank.is_empty():
				print("LCThrusterEffector: Fuel tank empty!")
	else:
		can_fire = false
	
	# Deplete oxidizer tank
	if oxidizer_tank:
		var oxidizer_available = oxidizer_tank.remove_resource(oxidizer_needed)
		if oxidizer_available < oxidizer_needed * 0.99:
			can_fire = false
			if oxidizer_tank.is_empty():
				print("LCThrusterEffector: Oxidizer tank empty!")
			# Refund fuel if oxidizer unavailable
			if fuel_tank and fuel_tank is LCResourceTankEffector:
				fuel_tank.add_resource(fuel_needed)
	elif mixture_ratio > 0:
		# No oxidizer tank but mixture ratio set - can't fire
		can_fire = false
	
	# Stop firing if insufficient propellant
	if not can_fire:
		current_thrust = 0.0
		is_firing = false
	
	# Calculate thrust direction with gimbal
	var thrust_dir = thrust_direction
	if can_vector and current_gimbal.length_squared() > 0:
		# Apply gimbal rotation
		var pitch_rad = deg_to_rad(current_gimbal.x)
		var yaw_rad = deg_to_rad(current_gimbal.y)
		
		# Rotate around local axes
		var pitch_basis = Basis(Vector3.RIGHT, pitch_rad)
		var yaw_basis = Basis(Vector3.UP, yaw_rad)
		thrust_dir = yaw_basis * pitch_basis * thrust_direction
	
	# Convert to global frame
	var global_force = local_to_global_force(thrust_dir * current_thrust)
	var application_point = global_position
	
	# if is_firing:
	# 	print("LCThrusterEffector: Force=", global_force.length(), " Dir=", thrust_dir)
	
	return {
		"force": global_force,
		"position": application_point,
		"torque": Vector3.ZERO  # Torque comes from offset application point
	}

## Returns current mass flow rate in kg/s.
func get_mass_flow_rate() -> float:
	if is_firing:
		return fuel_flow_rate * (current_thrust / max_thrust)
	return 0.0

## Returns total fuel consumed in kg.
func get_total_fuel_consumed() -> float:
	if specific_impulse > 0:
		return total_impulse / (specific_impulse * G0)
	return 0.0

func _initialize_telemetry():
	Telemetry = {
		"thrust_command": thrust_command,
		"current_thrust": current_thrust,
		"is_firing": is_firing,
		"firing_time": firing_time,
		"total_impulse": total_impulse,
		"gimbal_pitch": current_gimbal.x,
		"gimbal_yaw": current_gimbal.y,
		"fuel_flow_rate": get_mass_flow_rate(),
	}

func _update_telemetry():
	Telemetry["thrust_command"] = thrust_command
	Telemetry["current_thrust"] = current_thrust
	Telemetry["is_firing"] = is_firing
	Telemetry["firing_time"] = firing_time
	Telemetry["total_impulse"] = total_impulse
	Telemetry["gimbal_pitch"] = current_gimbal.x
	Telemetry["gimbal_yaw"] = current_gimbal.y
	Telemetry["fuel_flow_rate"] = get_mass_flow_rate()

# Command interface
func cmd_fire(args: Array):
	if args.size() >= 1:
		set_thrust(args[0])

func cmd_stop(args: Array):
	set_thrust(0.0)

func cmd_gimbal(args: Array):
	if args.size() >= 2:
		set_gimbal(args[0], args[1])
