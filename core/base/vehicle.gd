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
	_initialize_resource_network()
	_update_mass_properties()
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
			
		_manage_power_system(delta)
		_apply_effector_forces(delta)
		_apply_reaction_wheel_torques()


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
