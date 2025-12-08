@icon("res://core/base/space-system.svg")
class_name LCSpacecraft
extends RigidBody3D

## Base class for spacecraft using the effector-based component system.
## Handles mass aggregation, resource networks, and effector forces.

# Effector lists (auto-populated from children)
var state_effectors: Array = [] # Can hold LCStateEffector
var dynamic_effectors: Array = [] # Can hold LCDynamicEffector

# Resource solver (new physics)
var solver_graph: LCSolverGraph = null

# Aggregated properties (computed automatically)
var total_mass: float = 0.0
var power_consumption: float = 0.0
var power_production: float = 0.0
var power_available: float = 0.0

# Telemetry Schema (XTCE compatible mapping)
var Telemetry = {
	"position": "global_position",
	"velocity": "linear_velocity",
	"angular_velocity": "angular_velocity",
	"rotation": "global_rotation",
	"mass": "total_mass",
	"power_consumption": "power_consumption",
	"power_production": "power_production",
	"power_available": "power_available",
}

# Debug
@export var debug_effectors: bool = false
@export var center_of_mass_offset: Vector3 = Vector3.ZERO

# Optimization
var mass_properties_dirty: bool = true

func _ready():
	sleeping = false
	_discover_effectors()
	# Defer solver graph initialization to next frame so all effectors are ready
	call_deferred("_initialize_solver_graph")
	# Update mass after solver graph is initialized (when tanks have components)
	call_deferred("_update_mass_properties")
	_manage_power_system(0.0)

func _process(delta):
	if Engine.is_editor_hint():
		_update_mass_properties()

# Control inputs
var thrust_input: float = 0.0
var torque_input: Vector3 = Vector3.ZERO

## Sets control inputs from controller
func set_control_inputs(thrust: float, torque: Vector3):
	# print("LCSpacecraft: set_control_inputs thrust=", thrust, " torque=", torque)
	thrust_input = thrust
	torque_input = torque
	
	# Update pumps to control propellant flow
	for effector in dynamic_effectors:
		if effector.has_method("set_pump_power"):
			effector.set_pump_power(thrust_input)
	
	# Legacy: Update thrusters (now a no-op but kept for compatibility)
	for effector in dynamic_effectors:
		if effector is LCThrusterEffector:
			effector.set_thrust(thrust_input)

func get_telemetry_data() -> Dictionary:
	return { "Spacecraft": Telemetry }

func _physics_process(delta):
	# Only update if we have authority (for multiplayer)
	if is_multiplayer_authority():
		if mass_properties_dirty:
			_update_mass_properties()
			
		# Update solver graph
		if solver_graph:
			solver_graph.solve(delta)
			
		_manage_power_system(delta)
		_apply_effector_forces(delta)
		_apply_reaction_wheel_torques()
		_apply_control_torque(delta)
	else:
		# print("LCSpacecraft: No authority (", name, ")")
		pass

func _apply_control_torque(delta):
	if torque_input.length_squared() > 0.001:
		# print("LCSpacecraft: Applying torque ", torque_input)
		# If we have reaction wheels, they are handled in _apply_reaction_wheel_torques
		# But if we don't, or if we want "magic" torque from the controller:
		
		# Check if we have active reaction wheels
		var has_rw = false
		for effector in state_effectors:
			if effector is LCReactionWheelEffector:
				has_rw = true
				break
		
		if not has_rw:
			# Apply magic torque if no reaction wheels present
			# Scale torque by mass to make it usable across different ship sizes
			# A base turn rate of ~1 rad/s^2 requires Torque = Inertia * 1
			# Inertia approx 0.5 * mass * radius^2. Let's assume radius ~5m.
			# T = 0.5 * mass * 25 * 1 = 12.5 * mass.
			# Let's use a multiplier relative to mass.
			var magic_torque_mult = mass * 100.0 
			apply_torque(global_transform.basis * torque_input * magic_torque_mult)

## Callback from spacecraft controller
func _on_spacecraft_controller_thrusted(enabled: bool):
	# Legacy support or simple on/off
	set_control_inputs(1.0 if enabled else 0.0, torque_input)

## Initialize solver graph for new physics
func _initialize_solver_graph():
	if not solver_graph:
		solver_graph = LCSolverGraph.new()
	
	# Pass solver graph to all effectors
	for effector in state_effectors:
		if effector.has_method("set_solver_graph"):
			effector.set_solver_graph(solver_graph)
	
	for effector in dynamic_effectors:
		if effector not in state_effectors:  # Avoid double-calling
			if effector.has_method("set_solver_graph"):
				effector.set_solver_graph(solver_graph)
	
	if debug_effectors:
		print("[LCSpacecraft] Solver graph initialized")
		print("  Nodes: %d" % solver_graph.nodes.size())
		print("  Edges: %d" % solver_graph.edges.size())

## Discovers all effector children recursively.
func _discover_effectors():
	state_effectors.clear()
	dynamic_effectors.clear()
	
	for child in find_children("*"):
		if child is LCStateEffector:
			state_effectors.append(child)
			if not child.mass_changed.is_connected(_on_effector_mass_changed):
				child.mass_changed.connect(_on_effector_mass_changed)
			
		if child is LCDynamicEffector:
			dynamic_effectors.append(child)
	
	if debug_effectors:
		print("[LCSpacecraft] Discovered effectors:")
		for effector in state_effectors:
			print("  [State] %s (mass: %.2f kg)" % [effector.name, effector.get_mass_contribution()])
	
	mass_properties_dirty = true

func _on_effector_mass_changed():
	mass_properties_dirty = true

## Refresh effectors (call this after adding/removing children)
func refresh_effectors():
	_discover_effectors()
	_initialize_solver_graph()
	_update_mass_properties()

## Updates vehicle mass properties by aggregating from state effectors.
func _update_mass_properties():
	total_mass = 0.0
	var weighted_com = Vector3.ZERO
	
	for effector in state_effectors:
		if effector.has_method("get_mass_contribution"):
			var eff_mass = effector.get_mass_contribution()
			total_mass += eff_mass
			
			if effector.has_method("get_center_of_mass_offset"):
				weighted_com += effector.get_center_of_mass_offset() * eff_mass
			else:
				weighted_com += effector.position * eff_mass
	
	if total_mass > 1.0:
		mass = total_mass
		# Center of mass is relative to vehicle origin
		center_of_mass = (weighted_com / total_mass) + center_of_mass_offset
	else:
		mass = 1000.0 # Default for spacecraft
		center_of_mass = Vector3.ZERO
	
	mass_properties_dirty = false

## Manages power system with batteries and solar panels.
func _manage_power_system(delta: float):
	power_consumption = 0.0
	power_production = 0.0
	
	for effector in state_effectors:
		if effector.has_method("get_power_consumption"):
			power_consumption += effector.get_power_consumption()
		if effector.has_method("get_power_production"):
			power_production += effector.get_power_production()
	
	for effector in dynamic_effectors:
		if effector not in state_effectors:
			if effector.has_method("get_power_consumption"):
				power_consumption += effector.get_power_consumption()
			if effector.has_method("get_power_production"):
				power_production += effector.get_power_production()
	
	var net_power = power_production - power_consumption
	power_available = net_power

## Applies forces and torques from dynamic effectors.
func _apply_effector_forces(delta: float):
	for effector in dynamic_effectors:
		if effector.has_method("compute_force_torque"):
			var ft = effector.compute_force_torque(delta)
			
			if ft.has("force") and ft.force.length_squared() > 0:
				var position = ft.get("position", global_position)
				apply_force(ft.force, position - global_position)
			
			if ft.has("torque") and ft.torque.length_squared() > 0:
				apply_torque(ft.torque)

## Applies reaction torques from reaction wheels.
func _apply_reaction_wheel_torques():
	for effector in state_effectors:
		if effector is LCReactionWheelEffector:
			var rw_torque = effector.get_reaction_torque()
			if rw_torque.length_squared() > 0:
				apply_torque(rw_torque)
