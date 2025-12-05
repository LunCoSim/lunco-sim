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
		_update_visual_scale()

@export var max_motor_torque: float = 1000.0
@export var max_brake_force: float = 800.0

func _ready():
	# Apply properties
	wheel_radius = wheel_radius_config
	_update_visual_scale()
	
	# Initialize telemetry
	Telemetry["current_speed"] = 0.0
	Telemetry["motor_torque"] = 0.0
	Telemetry["brake_force"] = 0.0
	Telemetry["rpm"] = 0.0
	
	# Register Parameters for Universal Editor
	Parameters["Mass"] = { "path": "mass", "type": "float", "min": 1.0, "max": 100.0, "step": 1.0 }
	Parameters["Radius"] = { "path": "wheel_radius_config", "type": "float", "min": 0.1, "max": 2.0, "step": 0.05 }
	Parameters["Max Torque"] = { "path": "max_motor_torque", "type": "float", "min": 100.0, "max": 5000.0, "step": 100.0 }
	Parameters["Max Brake"] = { "path": "max_brake_force", "type": "float", "min": 100.0, "max": 5000.0, "step": 100.0 }

func _physics_process(delta):
	_update_solver_power()
	_update_telemetry()

var _accumulated_angle: float = 0.0

func _process(delta):
	# Update visual rotation
	var angular_velocity = 0.0
	
	if is_in_contact():
		# On ground: use real physics RPM
		# RPM * 2PI / 60 = rad/s
		angular_velocity = get_rpm() * TAU / 60.0
	else:
		# In air: simulate spin based on motor torque
		# Simple approximation: torque accelerates the wheel
		
		# If get_rpm() is very low but we have high torque, fake it.
		var rpm = get_rpm()
		
		# If we have significant torque request, spin up
		if abs(motor_torque_request) > 1.0:
			var target_vel = sign(motor_torque_request) * 20.0 # Arbitrary max speed
			angular_velocity = move_toward(_last_angular_velocity, target_vel, delta * 5.0)
		else:
			# Spin down friction
			angular_velocity = move_toward(_last_angular_velocity, 0.0, delta * 2.0)
			
			# If physics is still reporting something (e.g. just left ground), blend with it
			if abs(rpm) > 1.0:
				angular_velocity = rpm * TAU / 60.0

	_last_angular_velocity = angular_velocity
	_accumulated_angle += angular_velocity * delta
	
	# Keep angle within reasonable bounds to prevent float precision issues
	_accumulated_angle = fmod(_accumulated_angle, TAU)
	
	# Update shader parameter
	var mesh_instance = $MeshInstance3D
	if mesh_instance:
		var material = mesh_instance.get_surface_override_material(0)
		if material is ShaderMaterial:
			material.set_shader_parameter("angle", _accumulated_angle)

var _last_angular_velocity: float = 0.0

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

## Updates the visual mesh scale to match the wheel radius
func _update_visual_scale():
	var mesh_instance = $MeshInstance3D
	if mesh_instance:
		# The default cylinder mesh has radius 0.3
		# Scale it proportionally to match wheel_radius_config
		var scale_factor = wheel_radius_config / 0.3
		mesh_instance.scale = Vector3(scale_factor, scale_factor, scale_factor)

# --- Solver Integration ---
var solver_graph: LCSolverGraph
var solver_node: LCSolverNode

## Set the solver graph (called by vehicle during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not solver_node:
		# Create electrical load node
		# 0.0 potential means it's a passive node, but we'll drive it as a current sink
		solver_node = solver_graph.add_node(0.0, false, "Electrical")
		solver_node.resource_type = "electrical_power"
		solver_node.display_name = name
		
		# We act as a current sink (negative source)
		# The vehicle will connect us to the bus
		
		print("Wheel: Created solver node as power sink")

func _update_solver_power():
	if solver_node:
		# Calculate power consumption
		# Base consumption + motor load
		var current_power = power_consumption
		
		# Determine effective torque (either from custom request or Godot's engine_force)
		var effective_torque = motor_torque_request
		if abs(engine_force) > abs(effective_torque):
			effective_torque = engine_force
		
		# Add motor power: Torque * Angular Velocity
		# P = τ * ω
		if abs(effective_torque) > 0.1:
			var omega = get_wheel_rpm() * TAU / 60.0
			# Efficiency loss (heat) + Mechanical work
			# Simplified: Power = |Torque * Omega| / Efficiency
			# Let's assume 85% efficiency
			var motor_power = abs(effective_torque * omega) / 0.85
			current_power += motor_power
			
		# Update flow source (Amps)
		# I = P / V
		var bus_voltage = 28.0 # Default fallback
		if solver_node.potential > 1.0:
			bus_voltage = solver_node.potential
			
		var current_draw = current_power / bus_voltage
		
		# Negative flow source = consumption
		solver_node.flow_source = -current_draw
		
		# Update telemetry
		if Telemetry:
			Telemetry["bus_voltage"] = bus_voltage
			Telemetry["current_draw"] = current_draw
			Telemetry["actual_power"] = current_power

