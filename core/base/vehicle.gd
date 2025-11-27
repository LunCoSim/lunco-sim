@icon("res://core/base/space-system.svg")
class_name LCVehicle
extends VehicleBody3D

## Base class for all vehicles using the effector-based component system.
## ...

# Effector lists (auto-populated from children)
var state_effectors: Array = [] # Can hold LCStateEffector or LCWheelEffector
var dynamic_effectors: Array = [] # Can hold LCDynamicEffector or LCWheelEffector

# Resource network (auto-populated)
var resource_network: LCResourceNetwork = null

# Aggregated properties (computed automatically)
var total_mass: float = 0.0
var power_consumption: float = 0.0
var power_production: float = 0.0
var power_available: float = 0.0

# Telemetry (XTCE compatible)
var Telemetry: Dictionary = {}

# Debug
@export var debug_effectors: bool = false
@export var center_of_mass_offset: Vector3 = Vector3.ZERO # Manual offset for stability tuning

func _ready():
	_discover_effectors()
	_initialize_resource_network()
	_update_mass_properties()
	_manage_power_system(0.0)
	_initialize_telemetry()

func _process(delta):
	# Update mass in real-time if components change
	if Engine.is_editor_hint():
		_update_mass_properties()

func _physics_process(delta):
	# ... (existing logic)
	# Only update if we have authority (for multiplayer)
	if is_multiplayer_authority():
		_update_mass_properties()
		_manage_power_system(delta)
		_apply_effector_forces(delta)
		_apply_reaction_wheel_torques()
		_update_telemetry()

## Discovers all effector children recursively.
func _discover_effectors():
	state_effectors.clear()
	dynamic_effectors.clear()
	
	# Recursively find all effectors
	for child in find_children("*"):
		if child is LCStateEffector:
			state_effectors.append(child)
		elif child is LCWheelEffector:
			# LCWheelEffector implements the interface but extends VehicleWheel3D
			state_effectors.append(child)
			dynamic_effectors.append(child)
			
		if child is LCDynamicEffector:
			dynamic_effectors.append(child)
	
	if debug_effectors:
		print("[LCVehicle] Discovered effectors:")
		for effector in state_effectors:
			print("  [State] %s (mass: %.2f kg)" % [effector.name, effector.get_mass_contribution()])

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
			

## Manages power system with batteries and solar panels.
func _manage_power_system(delta: float):
	power_consumption = 0.0
	power_production = 0.0
	
	# Aggregate power from all effectors
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
	
	# Calculate net power
	var net_power = power_production - power_consumption
	
	# Manage battery charging/discharging
	for effector in state_effectors:
		if effector is LCBatteryEffector:
			if net_power > 0:
				# Charge battery with excess power
				effector.charge(net_power, delta)
			elif net_power < 0:
				# Discharge battery to meet demand
				var power_needed = abs(net_power)
				var power_delivered = effector.discharge(power_needed, delta)
				net_power += power_delivered
	
	power_available = net_power

## Applies forces and torques from dynamic effectors.
func _apply_effector_forces(delta: float):
	for effector in dynamic_effectors:
		if effector.has_method("compute_force_torque"):
			var ft = effector.compute_force_torque(delta)
			
			# Apply force if present
			if ft.has("force") and ft.force.length_squared() > 0:
				var position = ft.get("position", global_position)
				apply_force(ft.force, position - global_position)
			
			# Apply torque if present
			if ft.has("torque") and ft.torque.length_squared() > 0:
				apply_torque(ft.torque)

## Applies reaction torques from reaction wheels.
func _apply_reaction_wheel_torques():
	for effector in state_effectors:
		if effector is LCReactionWheelEffector:
			var rw_torque = effector.get_reaction_torque()
			if rw_torque.length_squared() > 0:
				apply_torque(rw_torque)


## Initializes telemetry dictionary.
func _initialize_telemetry():
	Telemetry = {
		"position": global_position,
		"velocity": linear_velocity,
		"angular_velocity": angular_velocity,
		"rotation": global_rotation,
		"mass": total_mass,
		"power_consumption": power_consumption,
		"power_production": power_production,
		"power_available": power_available,
	}

## Updates telemetry with current values.
func _update_telemetry():
	Telemetry["position"] = global_position
	Telemetry["velocity"] = linear_velocity
	Telemetry["angular_velocity"] = angular_velocity
	Telemetry["rotation"] = global_rotation
	Telemetry["mass"] = total_mass
	Telemetry["power_consumption"] = power_consumption
	Telemetry["power_production"] = power_production
	Telemetry["power_available"] = power_available

## Initialize resource network for automatic flow
func _initialize_resource_network():
	# Create network if it doesn't exist
	if not resource_network:
		resource_network = LCResourceNetwork.new()
		add_child(resource_network)
	
	# Rebuild network from current effectors
	resource_network.rebuild_from_vehicle(self)
	
	if debug_effectors:
		print("[LCVehicle] Resource network initialized with ", resource_network.nodes.size(), " nodes")
