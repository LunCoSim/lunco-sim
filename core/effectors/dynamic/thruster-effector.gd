class_name LCThrusterEffector
extends LCDynamicEffector

## Thruster dynamic effector that applies thrust forces and depletes fuel.
##
## Can be linked to a fuel tank for mass depletion.
## Supports thrust vectoring and on/off pulsing.
##
## PHYSICS UPDATE: 
## Acts as a vacuum sink (Ground Node, Potential=0) in the linear solver.
## Flow is controlled by upstream PUMPS, not by the thruster directly.
## Thrust is calculated from the ACTUAL mass flow rate provided by the pumps.

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
var fuel_edge: LCSolverEdge    ## Connection from fuel pump (read-only)
var oxidizer_edge: LCSolverEdge ## Connection from oxidizer pump (read-only)

@export_group("Thrust Vectoring")
@export var can_vector: bool = false  ## Can this thruster gimbal?
@export var max_gimbal_angle: float = 5.0  ## Maximum gimbal angle in degrees
@export var gimbal_rate: float = 10.0  ## Gimbal rate in degrees/second

@export_group("Performance")
@export var thrust_ramp_time: float = 0.1  ## Time to reach full thrust in seconds
@export var efficiency: float = 1.0  ## Thrust efficiency (0.0 to 1.0)

# Control inputs
var throttle_limit: float = 1.0 ## User-set max throttle (0.0 to 1.0) - safety limiter
var gimbal_command: Vector2 = Vector2.ZERO  ## Gimbal command in radians

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
	Parameters["Throttle Limit %"] = { "path": "throttle_limit", "type": "float", "min": 0.0, "max": 1.0, "step": 0.01, "category": "control" }
	
	# Read-only status displays
	Parameters["Firing"] = { "path": "is_firing", "type": "bool", "readonly": true, "category": "status" }
	Parameters["Current Thrust (N)"] = { "path": "current_thrust", "type": "float", "readonly": true, "category": "status" }
	Parameters["Mass Flow (kg/s)"] = { "path": "actual_mass_flow", "type": "float", "readonly": true, "category": "status" }

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
	
	# Auto-calculate fuel flow rate if not set (for reference only)
	if fuel_flow_rate <= 0.0 and specific_impulse > 0.0:
		var total_flow = max_thrust / (specific_impulse * G0)
		fuel_flow_rate = total_flow / (mixture_ratio + 1.0)  # Fuel portion only
		print("✓ LCThrusterEffector: Calculated fuel flow rate: ", fuel_flow_rate, " kg/s")
	
	# Set power consumption (rough estimate: 10W per 100N)
	power_consumption = max_thrust * 0.1
	
	_initialize_telemetry()

## Set the solver graph (called by vehicle during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and (fuel_tank or oxidizer_tank):
		# Create single engine node (Liquid domain to match tanks)
		# Represents the EXHAUST / VACUUM.
		# is_ground = true forces potential to 0.0 (Vacuum)
		solver_node = solver_graph.add_node(0.0, true, "Liquid")
		solver_node.display_name = name + " (Nozzle)"
		solver_node.resource_type = "combustion"
		
		# Note: Pumps will connect tanks to this engine node
		# We just need to identify our edges for flow reading
		# Edges will be created by pump effectors during their initialization

## Get the solver port (for pump connections)
func get_port() -> LCSolverNode:
	return solver_node

func _physics_process(delta):
	_update_gimbal(delta)
	_find_pump_edges()  # Identify edges from pumps
	# Force calculation happens in compute_force_torque

func _process(delta):
	_update_telemetry()

## Find edges connected to this engine (from pumps)
func _find_pump_edges():
	if not solver_node or fuel_edge or oxidizer_edge:
		return  # Already found or no solver node
	
	# Find edges connected to our engine node
	for edge in solver_node.edges:
		if edge.node_b == solver_node:  # Flow into engine
			# Determine if this is fuel or oxidizer based on source
			if not fuel_edge:
				fuel_edge = edge
			elif not oxidizer_edge:
				oxidizer_edge = edge

## Sets the thrust command (0.0 to 1.0) - for backward compatibility
## NOTE: With pump-based control, this doesn't directly control thrust
## Instead, use pumps to control flow. This is kept for legacy controller support.
func set_thrust(level: float):
	# For now, this is a no-op since pumps control flow
	# Could be used to adjust throttle_limit if desired
	pass

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
	var fuel_flow = 0.0
	var oxidizer_flow = 0.0
	
	if fuel_edge:
		fuel_flow = abs(fuel_edge.flow_rate)
		
	if oxidizer_edge:
		oxidizer_flow = abs(oxidizer_edge.flow_rate)
	
	# 2. CALCULATE COMBUSTIBLE FLOW based on mixture ratio
	# mixture_ratio = oxidizer/fuel (e.g., 3.6:1)
	# For complete combustion, need: oxidizer_flow = mixture_ratio × fuel_flow
	
	var combustible_fuel = 0.0
	var combustible_oxidizer = 0.0
	var excess_fuel = 0.0
	var excess_oxidizer = 0.0
	
	if fuel_flow > 0.001 and oxidizer_flow > 0.001:
		# Both propellants available - check ratio
		var actual_ratio = oxidizer_flow / fuel_flow
		
		if actual_ratio >= mixture_ratio:
			# Excess oxidizer - fuel limited
			combustible_fuel = fuel_flow
			combustible_oxidizer = fuel_flow * mixture_ratio
			excess_oxidizer = oxidizer_flow - combustible_oxidizer
		else:
			# Excess fuel - oxidizer limited
			combustible_oxidizer = oxidizer_flow
			combustible_fuel = oxidizer_flow / mixture_ratio
			excess_fuel = fuel_flow - combustible_fuel
	elif fuel_flow > 0.001:
		# Only fuel - all excess
		excess_fuel = fuel_flow
	elif oxidizer_flow > 0.001:
		# Only oxidizer - all excess
		excess_oxidizer = oxidizer_flow
	
	# 3. CALCULATE THRUST
	var combustion_flow = combustible_fuel + combustible_oxidizer
	var cold_gas_flow = excess_fuel + excess_oxidizer
	
	# Physics-based Isp calculation from chamber conditions
	var effective_isp = _calculate_isp(combustion_flow, cold_gas_flow, fuel_flow, oxidizer_flow)
	
	# Combustion thrust: uses calculated Isp
	var combustion_thrust = combustion_flow * effective_isp * G0
	
	# Cold gas thrust: excess propellant, much lower Isp (~50s for cold gas)
	var cold_gas_isp = 50.0  # Typical for cold gas thrusters
	var cold_gas_thrust = cold_gas_flow * cold_gas_isp * G0
	
	# Total thrust
	current_thrust = combustion_thrust + cold_gas_thrust
	actual_mass_flow = combustion_flow + cold_gas_flow
	
	# 4. APPLY THROTTLE LIMIT (safety cap)
	current_thrust *= throttle_limit
	
	# Update State
	is_firing = current_thrust > 0.01
	visible = is_firing # Show plume if thrusting
	
	if is_firing:
		firing_time += delta
		total_impulse += current_thrust * delta
	
	# 4. APPLY FORCE
	if not is_firing:
		return {
			"force": Vector3.ZERO,
			"position": global_position,
			"torque": Vector3.ZERO
		}
	
	# Apply gimbal
	var thrust_dir = thrust_direction
	if can_vector:
		var pitch_rad = deg_to_rad(current_gimbal.x)
		var yaw_rad = deg_to_rad(current_gimbal.y)
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

## Calculate Isp based on chamber pressure and propellant properties
func _calculate_isp(combustion_flow: float, cold_gas_flow: float, fuel_flow: float, oxidizer_flow: float) -> float:
	if combustion_flow < 0.001:
		return 50.0  # Cold gas only
	
	# Estimate chamber pressure from pump pressure and flow
	# Higher flow → higher chamber pressure
	var chamber_pressure = 0.0
	if fuel_edge and oxidizer_edge:
		# Average of fuel and oxidizer pressures
		chamber_pressure = (abs(fuel_edge.node_a.potential) + abs(oxidizer_edge.node_a.potential)) / 2.0
	
	# Clamp to reasonable range (1-300 bar)
	chamber_pressure = clamp(chamber_pressure, 100000.0, 30000000.0)  # 1-300 bar in Pa
	
	# Simplified rocket equation: Isp ∝ sqrt(T_chamber / M_molecular)
	# For methane/oxygen combustion:
	# - Combustion temperature: ~3500 K
	# - Molecular weight: ~23 g/mol (CO2 + H2O mix)
	# - Expansion ratio depends on chamber pressure
	
	var combustion_temp = 3500.0  # K (typical for CH4/O2)
	var molecular_weight = 23.0  # g/mol
	
	# Expansion efficiency increases with chamber pressure
	# At 1 bar: ~0.5, at 100 bar: ~0.9, at 300 bar: ~0.95
	var pressure_bar = chamber_pressure / 100000.0
	var expansion_efficiency = 0.5 + 0.45 * (1.0 - exp(-pressure_bar / 50.0))
	
	# Theoretical Isp = expansion_efficiency × sqrt(T/M) × constant
	# Constant ≈ 10 for this simplified model
	var theoretical_isp = expansion_efficiency * sqrt(combustion_temp / molecular_weight) * 10.0
	
	# Clamp to reasonable range (200-400s for CH4/O2)
	return clamp(theoretical_isp, 200.0, 400.0)

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
		"current_thrust": current_thrust,
		"is_firing": is_firing,
		"firing_time": firing_time,
		"total_impulse": total_impulse,
		"gimbal_pitch": current_gimbal.x,
		"gimbal_yaw": current_gimbal.y,
		"mass_flow_rate": actual_mass_flow,
	}

func _update_telemetry():
	Telemetry["current_thrust"] = current_thrust
	Telemetry["is_firing"] = is_firing
	Telemetry["firing_time"] = firing_time
	Telemetry["total_impulse"] = total_impulse
	Telemetry["gimbal_pitch"] = current_gimbal.x
	Telemetry["gimbal_yaw"] = current_gimbal.y
	Telemetry["mass_flow_rate"] = actual_mass_flow

# Command interface
func cmd_gimbal(args: Array):
	if args.size() >= 2:
		set_gimbal(args[0], args[1])

func cmd_set_throttle_limit(args: Array):
	if args.size() >= 1:
		throttle_limit = clamp(args[0], 0.0, 1.0)
