@icon("res://core/base/space-system.svg")
class_name LCVehicle
extends VehicleBody3D

## Base class for all vehicles using the effector-based component system.
## ...

# Effector lists (auto-populated from children)
var state_effectors: Array = [] # Can hold LCStateEffector or LCWheelEffector
var dynamic_effectors: Array = [] # Can hold LCDynamicEffector or LCWheelEffector

# Control routing map: action_name -> Array of effectors
var control_map: Dictionary = {}

# Resource solver (new physics)
var solver_graph: LCSolverGraph = null

# Aggregated properties (computed automatically)
var total_mass: float = 0.0
var power_consumption: float = 0.0
var power_production: float = 0.0
var power_available: float = 0.0

# Telemetry Schema (XTCE compatible mapping)
# Maps telemetry keys to property names
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
@export var center_of_mass_offset: Vector3 = Vector3.ZERO # Manual offset for stability tuning

# Optimization
var mass_properties_dirty: bool = true

func _ready():
	_discover_effectors()
	# Defer solver graph initialization to next frame so all effectors are ready
	call_deferred("_initialize_solver_graph")
	# Update mass after solver graph is initialized (when tanks have components)
	call_deferred("_update_mass_properties")
	_manage_power_system(0.0)

func _process(delta):
	# Update mass in real-time if components change
	if Engine.is_editor_hint():
		_update_mass_properties()

func _physics_process(delta):
	# ... (existing logic)
	# Only update if we have authority (for multiplayer)
	if is_multiplayer_authority():
		if mass_properties_dirty:
			_update_mass_properties()
			
		# Update solver graph
		if solver_graph:
			solver_graph.solve(delta)
			
		_manage_power_system(delta)
		_apply_effector_forces(delta)


## Initialize solver graph for physics
func _initialize_solver_graph():
	if not solver_graph:
		solver_graph = LCSolverGraph.new()
	
	# Create electrical bus (common node for all electrical components)
	var electrical_bus = solver_graph.add_node(28.0, false, "Electrical")
	electrical_bus.resource_type = "electrical_power"
	electrical_bus.display_name = "Electrical Bus"
	
	# Pass solver graph to all effectors
	for effector in state_effectors:
		if effector.has_method("set_solver_graph"):
			effector.set_solver_graph(solver_graph)
	
	for effector in dynamic_effectors:
		if effector not in state_effectors:  # Avoid double-calling
			if effector.has_method("set_solver_graph"):
				effector.set_solver_graph(solver_graph)
	
	# Connect all electrical components to the bus
	# Use high conductance (low resistance) for wiring
	var wire_conductance = 1000.0  # Very low resistance
	
	for effector in state_effectors:
		if effector.has_method("set_solver_graph") and effector.get("solver_node"):
			var node = effector.solver_node
			if node and node.domain == "Electrical" and node != electrical_bus:
				solver_graph.connect_nodes(electrical_bus, node, wire_conductance, "Electrical")
	
	for effector in dynamic_effectors:
		if effector not in state_effectors:
			if effector.has_method("set_solver_graph") and effector.get("solver_node"):
				var node = effector.solver_node
				if node and node.domain == "Electrical" and node != electrical_bus:
					solver_graph.connect_nodes(electrical_bus, node, wire_conductance, "Electrical")
	
	if debug_effectors:
		print("[LCVehicle] Solver graph initialized")
		print("  Nodes: %d" % solver_graph.nodes.size())
		print("  Edges: %d" % solver_graph.edges.size())

## Discovers all effector children recursively.
func _discover_effectors():
	state_effectors.clear()
	dynamic_effectors.clear()
	control_map.clear()
	
	# Recursively find all effectors using duck typing
	for child in find_children("*"):
		# Does it behave like a state effector (has mass)?
		if child.has_method("get_mass_contribution"):
			state_effectors.append(child)
			if child.has_signal("mass_changed"):
				if not child.mass_changed.is_connected(_on_effector_mass_changed):
					child.mass_changed.connect(_on_effector_mass_changed)
			
		# Does it behave like a dynamic effector (has force)?
		if child.has_method("compute_force_torque"):
			dynamic_effectors.append(child)
			
		# Register control actions
		if child.has_method("get_control_actions"):
			var actions = child.get_control_actions()
			for action in actions:
				if not control_map.has(action):
					control_map[action] = []
				control_map[action].append(child)
	
	if debug_effectors:
		print("[LCVehicle] Discovered effectors:")
		for effector in state_effectors:
			print("  [State] %s (mass: %.2f kg)" % [effector.name, effector.get_mass_contribution()])
		print("  [Control] Registered actions: ", control_map.keys())
	
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
	
	# Aggregate mass and center of mass
	for effector in state_effectors:
		if effector.has_method("get_mass_contribution"):
			var eff_mass = effector.get_mass_contribution()
			total_mass += eff_mass
			
			if effector.has_method("get_center_of_mass_offset"):
				weighted_com += effector.get_center_of_mass_offset() * eff_mass
			else:
				weighted_com += effector.position * eff_mass
	
	# Update physics body properties
	if total_mass > 1.0:
		mass = total_mass
		# Center of mass is relative to vehicle origin
		center_of_mass = (weighted_com / total_mass) + center_of_mass_offset
	else:
		# Fallback to prevent physics explosion
		mass = 50.0 
		center_of_mass = Vector3.ZERO
		if debug_effectors:
			push_warning("[LCVehicle] Total mass is too low (%.2f kg). Defaulting to 50.0 kg." % total_mass)
			
	mass_properties_dirty = false

## Manages power system with batteries and solar panels.
## Manages power system with batteries and solar panels.
func _manage_power_system(delta: float):
	power_consumption = 0.0
	power_production = 0.0
	
	# Aggregate power from all effectors for telemetry
	for effector in state_effectors:
		if effector.has_method("get_power_consumption"):
			power_consumption += effector.get_power_consumption()
		if effector.has_method("get_power_production"):
			power_production += effector.get_power_production()
	
	for effector in dynamic_effectors:
		if effector not in state_effectors: # Avoid double counting if hybrid
			if effector.has_method("get_power_consumption"):
				power_consumption += effector.get_power_consumption()
			if effector.has_method("get_power_production"):
				power_production += effector.get_power_production()
	
	var net_power = power_production - power_consumption
	power_available = net_power

## Applies forces and torques from dynamic effectors.
func _apply_effector_forces(delta: float):
	for effector in dynamic_effectors:
		# compute_force_torque is guaranteed by _discover_effectors check
		var ft = effector.compute_force_torque(delta)
		
		# Apply force if present
		if ft.has("force") and ft.force.length_squared() > 0:
			var position = ft.get("position", global_position)
			apply_force(ft.force, position - global_position)
		
		# Apply torque if present
		if ft.has("torque") and ft.torque.length_squared() > 0:
			apply_torque(ft.torque)
