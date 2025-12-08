class_name LCThrusterEffector
extends LCDynamicEffector

## Thruster dynamic effector that applies thrust forces and depletes fuel.
##
## Can be linked to a fuel tank for mass depletion.
## Supports thrust vectoring and on/off pulsing.
##
## PHYSICS UPDATE: 
## Acts as a vacuum sink (Ground Node, Potential=0) in the linear solver.
## Thrust command modulates the CONDUCTANCE of the connection to the tanks (Valve).
## Thrust is calculated from the ACTUAL mass flow rate resulting from reservoir pressure.

@export_group("Thruster Properties")
@export var max_thrust: float = 100.0  ## Maximum thrust in Newtons
@export var specific_impulse: float = 300.0  ## Isp in seconds
@export var min_on_time: float = 0.02  ## Minimum firing pulse in seconds
@export var thrust_direction: Vector3 = Vector3(0, 0, 1)  ## Local thrust direction (normalized)

@export_group("Propellant Connection")
@export var fuel_tank_path: NodePath
var fuel_tank = null  ## LCResourceTankEffector
@export var oxidizer_tank_path: NodePath
var oxidizer_tank = null  ## LCResourceTankEffector for oxygen
@export var mixture_ratio: float = 3.6  ## Oxidizer:Fuel mass ratio (3.6:1 for Starship Raptor)
@export var fuel_flow_rate: float = 0.0  ## kg/s at max thrust (auto-calculated if 0)

# Solver Integration
var solver_graph: LCSolverGraph
var solver_node: LCSolverNode  ## Single engine node (acts as exhaust/vacuum)
var fuel_edge: LCSolverEdge    ## Connection to fuel tank (valve)
var oxidizer_edge: LCSolverEdge ## Connection to oxidizer tank (valve)
var valve_max_conductance: float = 1.0  ## Calculated conductance when fully open

@export_group("Thrust Vectoring")
@export var can_vector: bool = false  ## Can this thruster gimbal?
@export var max_gimbal_angle: float = 5.0  ## Maximum gimbal angle in degrees
@export var gimbal_rate: float = 10.0  ## Gimbal rate in degrees/second

@export_group("Performance")
@export var thrust_ramp_time: float = 0.1  ## Time to reach full thrust in seconds
@export var efficiency: float = 1.0  ## Thrust efficiency (0.0 to 1.0)

# Control inputs
var throttle_limit: float = 1.0 ## User-set max throttle (0.0 to 1.0)
var thrust_command: float = 0.0  ## Commanded thrust level (0.0 to 1.0)
var gimbal_command: Vector2 = Vector2.ZERO  ## Gimbal command in radians
var current_throttle: float = 0.0 ## Actual valve position (0.0 to 1.0) due to ramping

# Internal state
var current_thrust: float = 0.0  ## Current actual thrust
var current_gimbal: Vector2 = Vector2.ZERO  ## Current gimbal angles
var is_firing: bool = false
var firing_time: float = 0.0
var total_impulse: float = 0.0  ## Total impulse delivered in N·s
var actual_mass_flow: float = 0.0 ## Current total mass flow (kg/s)

# Physics constants
const G0: float = 9.80665  ## Standard gravity in m/s²
# Estimation pressure for calculating conductance (e.g., 3 bar typical tank pressure)
# This is used to size the "valve" so that at nominal pressure, we get max thrust.
const NOMINAL_TANK_PRESSURE: float = 300000.0 # 3 Bar

func _init():
	_initialize_parameters()

func _initialize_parameters():
	# User-controllable settings
	Parameters["Throttle Limit %"] = { "path": "throttle_limit", "type": "float", "min": 0.0, "max": 1.0, "step": 0.01 }
	
	# Read-only status displays
	Parameters["Firing"] = { "path": "is_firing", "type": "bool", "readonly": true }
	Parameters["Current Thrust (N)"] = { "path": "current_thrust", "type": "float", "readonly": true }
	Parameters["Mass Flow (kg/s)"] = { "path": "actual_mass_flow", "type": "float", "readonly": true }

func _ready():
	super._ready()
	thrust_direction = thrust_direction.normalized()
	
	# Connect to fuel tank
	if not fuel_tank_path.is_empty():
		var node = get_node_or_null(fuel_tank_path)
		if node is LCResourceTankEffector:
			fuel_tank = node
			print("✓ LCThrusterEffector: Connected to fuel tank ", node.name, " with ", node.get_amount(), " kg")
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
	if fuel_flow_rate <= 0.0 and specific_impulse > 0.0:
		var total_flow = max_thrust / (specific_impulse * G0)
		fuel_flow_rate = total_flow / (mixture_ratio + 1.0)  # Fuel portion only
		print("✓ LCThrusterEffector: Calculated fuel flow rate: ", fuel_flow_rate, " kg/s")
		
		# Calculate Max Conductance (Valve Size)
		# Flow = Conductance * Pressure
		# Conductance = Flow / Pressure
		# We assume flow drives towards vacuum (0 pressure), so DeltaP = TankPressure
		valve_max_conductance = total_flow / NOMINAL_TANK_PRESSURE
		print("✓ LCThrusterEffector: Calculated valve Kv: ", valve_max_conductance)
	
	# Set power consumption (rough estimate: 10W per 100N)
	power_consumption = max_thrust * 0.1
	
	_initialize_telemetry()

## Set the solver graph (called by vehicle during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and (fuel_tank or oxidizer_tank):
		# Create single engine node (Fluid domain)
		# Represents the EXHAUST / VACUUM.
		# is_ground = true forces potential to 0.0 (Vacuum)
		solver_node = solver_graph.add_node(0.0, true, "Fluid")
		solver_node.display_name = name + " (Nozzle)"
		solver_node.resource_type = "combustion"
		
		# Connect to fuel tank
		if fuel_tank:
			var fuel_port = fuel_tank.get_port()
			if fuel_port:
				# Connect with 0 conductance initially (Closed Valve)
				fuel_edge = solver_graph.connect_nodes(fuel_port, solver_node, 0.0, "Fluid")
				fuel_edge.is_unidirectional = true # Check valve, only flow out
		
		# Connect to oxidizer tank
		if oxidizer_tank:
			var oxidizer_port = oxidizer_tank.get_port()
			if oxidizer_port:
				# Connect with 0 conductance initially (Closed Valve)
				oxidizer_edge = solver_graph.connect_nodes(oxidizer_port, solver_node, 0.0, "Fluid")
				oxidizer_edge.is_unidirectional = true # Check valve, only flow out

func _physics_process(delta):
	_update_throttle_ramp(delta)
	_update_solver_valves()
	_update_gimbal(delta)
	# Force calculation happens in compute_force_torque

func _process(delta):
	_update_telemetry()

## Sets the thrust command (0.0 to 1.0).
func set_thrust(level: float):
	# Apply throttle limit to the input command
	thrust_command = clamp(level, 0.0, 1.0) * throttle_limit

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

## Updates throttle level with ramping.
func _update_throttle_ramp(delta: float):
	var target = thrust_command * efficiency
	
	# Ramp throttle
	if thrust_ramp_time > 0:
		var ramp_rate = 1.0 / thrust_ramp_time
		if current_throttle < target:
			current_throttle = min(current_throttle + ramp_rate * delta, target)
		else:
			current_throttle = max(current_throttle - ramp_rate * delta, target)
	else:
		current_throttle = target

## Updates solver edge conductance based on throttle
func _update_solver_valves():
	if not solver_graph: return
	
	var current_conductance = valve_max_conductance * current_throttle
	
	# If mixture ratio is used, we split conductance accordingly?
	# Or simplified: Both valves open proportionally. 
	# Refined: To maintain mixture ratio, conductance should be proportional to mass fractions.
	# But if tank pressures are equal, conductance ratio = mass flow ratio.
	
	# Fraction of total flow for each prop
	var fuel_fraction = 1.0 / (mixture_ratio + 1.0)
	var ox_fraction = mixture_ratio / (mixture_ratio + 1.0)
	
	if fuel_edge:
		fuel_edge.conductance = current_conductance * fuel_fraction
	
	if oxidizer_edge:
		oxidizer_edge.conductance = current_conductance * ox_fraction

## Updates gimbal angles.
func _update_gimbal(delta: float):
	if can_vector:
		# Ramp gimbal angles
		var gimbal_delta = gimbal_rate * delta
		current_gimbal.x = move_toward(current_gimbal.x, gimbal_command.x, gimbal_delta)
		current_gimbal.y = move_toward(current_gimbal.y, gimbal_command.y, gimbal_delta)

## Computes force and torque from thruster.
func compute_force_torque(delta: float) -> Dictionary:
	# 1. READ ACTUAL FLOW from Solver
	var total_flow = 0.0
	
	if fuel_edge:
		total_flow += fuel_edge.flow_rate
		
	if oxidizer_edge:
		total_flow += oxidizer_edge.flow_rate
	
	actual_mass_flow = total_flow
	
	# 2. CALCULATE THRUST from Physics
	# F = m_dot * Isp * g0
	current_thrust = actual_mass_flow * specific_impulse * G0
	
	# Update State
	is_firing = current_thrust > 0.01
	visible = is_firing # Show plume if thrusting
	
	if is_firing:
		firing_time += delta
		total_impulse += current_thrust * delta
	
	# 3. APPLY FORCE
	if not is_firing:
		return {}
	
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
	
	return {
		"force": global_force,
		"position": application_point,
		"torque": Vector3.ZERO  # Torque comes from offset application point
	}

## Returns current mass flow rate in kg/s.
func get_mass_flow_rate() -> float:
	return actual_mass_flow

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
		"mass_flow_rate": actual_mass_flow,
	}

func _update_telemetry():
	Telemetry["thrust_command"] = thrust_command
	Telemetry["current_thrust"] = current_thrust
	Telemetry["is_firing"] = is_firing
	Telemetry["firing_time"] = firing_time
	Telemetry["total_impulse"] = total_impulse
	Telemetry["gimbal_pitch"] = current_gimbal.x
	Telemetry["gimbal_yaw"] = current_gimbal.y
	Telemetry["mass_flow_rate"] = actual_mass_flow

# Command interface
func cmd_fire(args: Array):
	if args.size() >= 1:
		set_thrust(args[0])

func cmd_stop(args: Array):
	set_thrust(0.0)

func cmd_gimbal(args: Array):
	if args.size() >= 2:
		set_gimbal(args[0], args[1])
