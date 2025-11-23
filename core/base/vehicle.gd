@icon("res://core/base/space-system.svg")
class_name LCVehicle
extends VehicleBody3D

## Base class for all vehicles using the effector-based component system.
## ...

# Effector lists (auto-populated from children)
var state_effectors: Array = [] # Can hold LCStateEffector or LCWheelEffector
var dynamic_effectors: Array = [] # Can hold LCDynamicEffector or LCWheelEffector

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
	_update_mass_properties()
	_update_power_budget()
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
		_update_power_budget()
		_apply_effector_forces(delta)
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
			
	if debug_effectors:
		print("[LCVehicle] Updated mass: %.2f kg, CoM: %s" % [mass, center_of_mass])

## Updates power budget by aggregating from all effectors.
func _update_power_budget():
	power_consumption = 0.0
	power_production = 0.0
	
	# Aggregate from all effectors
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
	
	power_available = power_production - power_consumption

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

## Initializes telemetry dictionary.
func _initialize_telemetry():
	Telemetry = {
		"total_mass": total_mass,
		"power_consumption": power_consumption,
		"power_production": power_production,
		"power_available": power_available,
		"position": global_position,
		"velocity": linear_velocity,
		"angular_velocity": angular_velocity,
	}

## Updates telemetry with current values.
func _update_telemetry():
	Telemetry["total_mass"] = total_mass
	Telemetry["power_consumption"] = power_consumption
	Telemetry["power_production"] = power_production
	Telemetry["power_available"] = power_available
	Telemetry["position"] = global_position
	Telemetry["velocity"] = linear_velocity
	Telemetry["angular_velocity"] = angular_velocity
