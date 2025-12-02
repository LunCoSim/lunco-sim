class_name LCResourceTankEffector
extends LCStateEffector

## Universal resource storage tank
##
## Can store any resource type defined in the resource registry.
## Replaces specific tank types (fuel tank, oxygen tank, etc.)

@export var resource_id: String = "oxygen"  ## Resource type to store
@export var capacity: float = 100.0  ## Maximum capacity in resource units (kg, L, kWh, etc.)
@export var tank_dry_mass: float = 10.0  ## Mass of empty tank

var resource_container: LCResourceContainer
var is_initialized: bool = false

func _ready():
	super._ready()
	_initialize_tank()

func _initialize_tank():
	if is_initialized:
		return
	
	# Get resource definition from registry
	var res_def = LCResourceRegistry.get_resource(resource_id)
	if res_def:
		# Initialize with current_amount (set from scene properties)
		var initial_amount = 0.0
		if has_meta("initial_amount"):
			initial_amount = get_meta("initial_amount")
		resource_container = LCResourceContainer.new(res_def, initial_amount)
		is_initialized = true
		print("ResourceTank: Initialized for ", res_def.display_name, " with ", initial_amount, " kg")
	else:
		push_error("ResourceTank: Resource not found in registry: " + resource_id)
		# Create a placeholder to prevent crashes
		var placeholder = LCResourceDefinition.new()
		placeholder.resource_id = resource_id
		placeholder.display_name = "Unknown (" + resource_id + ")"
		resource_container = LCResourceContainer.new(placeholder, 0.0)
	
	_update_mass()
	_initialize_telemetry()
	_initialize_parameters()

var current_amount: float:
	get:
		return get_amount()
	set(value):
		set_amount(value)

var fill_percentage_param: float:
	get:
		return get_fill_percentage()
	set(value):
		set_amount(capacity * value / 100.0)

func _initialize_parameters():
	# Capacity and dry mass use reasonable kg ranges
	Parameters["Capacity"] = { "path": "capacity", "type": "float", "min": 1000.0, "max": 2000000.0, "step": 1000.0 }
	Parameters["Dry Mass"] = { "path": "tank_dry_mass", "type": "float", "min": 1000.0, "max": 100000.0, "step": 1000.0 }
	# Amount uses percentage (0-100%) for better slider UX
	Parameters["Fill %"] = { "path": "fill_percentage_param", "type": "float", "min": 0.0, "max": 100.0, "step": 0.1 }
	# Direct text input for exact mass
	Parameters["Amount (kg)"] = { "path": "current_amount", "type": "float", "text_field": true }

## Add resource to tank
func add_resource(amount: float) -> float:
	if not resource_container:
		return 0.0
	
	var space_available = capacity - resource_container.amount
	var added = resource_container.add(min(amount, space_available))
	_update_mass()
	return added

## Remove resource from tank
func remove_resource(amount: float) -> float:
	if not resource_container:
		return 0.0
	
	var removed = resource_container.remove(amount)
	_update_mass()
	return removed

## Transfer resource to another tank
func transfer_to(other_tank: LCResourceTankEffector, amount: float) -> float:
	if not other_tank or not resource_container or not other_tank.resource_container:
		return 0.0
	
	# Check if same resource type
	if other_tank.resource_id != resource_id:
		push_error("Cannot transfer between different resource types")
		return 0.0
	
	# Calculate how much can be transferred
	var available = resource_container.amount
	var space_in_other = other_tank.capacity - other_tank.resource_container.amount
	var transfer_amount = min(amount, min(available, space_in_other))
	
	if transfer_amount > 0:
		var transferred = resource_container.transfer_to(other_tank.resource_container, transfer_amount)
		_update_mass()
		other_tank._update_mass()
		return transferred
	
	return 0.0

## Get current fill percentage
func get_fill_percentage() -> float:
	if not resource_container:
		return 0.0
	return resource_container.get_fill_percentage(capacity)

## Get current amount
func get_amount() -> float:
	if not resource_container:
		return 0.0
	return resource_container.amount

## Set amount (for initialization)
func set_amount(new_amount: float):
	if not resource_container:
		# Store for later initialization
		set_meta("initial_amount", new_amount)
		return
	resource_container.amount = clamp(new_amount, 0.0, capacity)
	_update_mass()

## Check if tank is empty
func is_empty() -> bool:
	if not resource_container:
		return true
	return resource_container.is_empty()

## Check if tank is full
func is_full() -> bool:
	if not resource_container:
		return false
	return resource_container.amount >= capacity * 0.99  # 99% is "full"

## Get resource name
func get_resource_name() -> String:
	if resource_container:
		return resource_container.get_resource_name()
	return "Unknown"

## Update vehicle mass based on tank contents
func _update_mass():
	if resource_container:
		mass = tank_dry_mass + resource_container.get_mass()
	else:
		mass = tank_dry_mass

func _initialize_telemetry():
	Telemetry = {
		"resource_id": resource_id,
		"resource_name": get_resource_name(),
		"amount": get_amount(),
		"capacity": capacity,
		"fill_percentage": get_fill_percentage(),
		"mass": mass,
		"temperature": resource_container.temperature if resource_container else 293.15,
	}

func _physics_process(delta):
	_update_telemetry()

func _update_telemetry():
	if not Telemetry:
		return
	
	Telemetry["amount"] = get_amount()
	Telemetry["fill_percentage"] = get_fill_percentage()
	Telemetry["mass"] = mass
	if resource_container:
		Telemetry["temperature"] = resource_container.temperature

# Command interface for remote control
func cmd_fill(args: Array):
	if args.size() > 0:
		set_amount(args[0])

func cmd_empty(args: Array):
	set_amount(0.0)

func cmd_add(args: Array):
	if args.size() > 0:
		add_resource(args[0])

func cmd_remove(args: Array):
	if args.size() > 0:
		remove_resource(args[0])
