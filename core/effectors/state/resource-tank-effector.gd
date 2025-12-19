class_name LCResourceTankEffector
extends LCStateEffector

## Universal resource storage tank (New Physics)
##
## Uses LCTankComponent for pressure-based bidirectional flow.
## Replaces the old LCResourceContainer system.

@export var resource_id: String = "oxygen"  ## Resource type to store
@export var capacity: float = 100.0  ## Maximum capacity in kg
@export var tank_dry_mass: float = 10.0  ## Mass of empty tank in kg
@export var tank_height: float = 2.0  ## Physical height for pressure calculation in m
@export var initial_fill_percentage: float = 100.0  ## Initial fill level (0-100%)

# Physics component
var component: LCTankComponent
# Inherited from LCStateEffector:
# var solver_graph: LCSolverGraph

var is_initialized: bool = false

func _ready():
	super._ready()
	_initialize_tank()

func _initialize_tank():
	if is_initialized:
		return
	
	# Get resource definition from registry
	# Use dynamic lookup to avoid compile-time dependency issues with AutoLoad
	var registry = get_node_or_null("/root/LCResourceRegistry")
	if not registry:
		push_error("ResourceTank: LCResourceRegistry not found at /root/LCResourceRegistry")
		return

	var res_def = registry.get_resource(resource_id)
	if not res_def:
		push_error("ResourceTank: Resource not found in registry: " + resource_id)
		return
	
	is_initialized = true
	print("ResourceTank: Initialized for ", res_def.display_name)
	
	_initialize_telemetry()
	_initialize_parameters()

## Set the solver graph (called by spacecraft during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not component:
		# Get resource density
		var registry = get_node_or_null("/root/LCResourceRegistry")
		var res_def = registry.get_resource(resource_id) if registry else null
		var density = res_def.density if res_def else 1000.0
		
		# Create component
		var volume = capacity / density
		component = LCTankComponent.new(solver_graph, volume, tank_height, density)
		
		# Set initial mass
		var initial_mass = (initial_fill_percentage / 100.0) * capacity
		component.set_initial_mass(initial_mass)
		
		# Set display name and resource type on the solver node
		var port = component.get_port("port")
		if port:
			port.display_name = name  # Use effector's name
			port.resource_type = resource_id
			port.effector_ref = weakref(self)
		
		_update_mass()

# --- Public API ---

## Get current amount in kg
func get_amount() -> float:
	if component:
		return component.mass
	return 0.0

## Get current fill percentage (0-100)
func get_fill_percentage() -> float:
	if component:
		var max_mass = component.volume * component.density
		return (component.mass / max_mass) * 100.0 if max_mass > 0 else 0.0
	return 0.0

## Set amount in kg (for initialization/commands)
func set_amount(new_amount: float):
	if component:
		component.set_initial_mass(new_amount)
		_update_mass()

## Add resource to tank (helper)
func add_resource(amount: float):
	if component:
		component.mass += amount
		_update_mass()

## Remove resource from tank (helper)
## Returns actual amount removed
func remove_resource(amount: float) -> float:
	if component:
		var actual_remove = min(amount, component.mass)
		component.mass -= actual_remove
		_update_mass()
		return actual_remove
	return 0.0

## Get the solver port (for manual connections)
func get_port() -> LCSolverNode:
	if component:
		return component.get_port("port")
	return null

## Check if tank is empty
func is_empty() -> bool:
	if component:
		return component.mass < 0.1
	return true

## Check if tank is full
func is_full() -> bool:
	if component:
		var max_mass = component.volume * component.density
		return component.mass >= max_mass * 0.99
	return false

## Get resource name
func get_resource_name() -> String:
	var registry = get_node_or_null("/root/LCResourceRegistry")
	var res_def = registry.get_resource(resource_id) if registry else null
	return res_def.display_name if res_def else "Unknown"

# --- Internal ---

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
	Parameters["Capacity"] = { "path": "capacity", "type": "float", "min": 1000.0, "max": 2000000.0, "step": 1000.0 }
	Parameters["Dry Mass"] = { "path": "tank_dry_mass", "type": "float", "min": 1000.0, "max": 100000.0, "step": 1000.0 }
	Parameters["Fill %"] = { "path": "fill_percentage_param", "type": "float", "min": 0.0, "max": 100.0, "step": 0.1 }
	Parameters["Amount (kg)"] = { "path": "current_amount", "type": "float", "text_field": true }

func _update_mass():
	if component:
		mass = tank_dry_mass + component.mass
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
		"pressure": 0.0,
	}

func _process(delta):
	_update_telemetry()

func _physics_process(delta):
	# Update component physics
	if component:
		component.update(delta)
		_update_mass()

func _update_telemetry():
	if not Telemetry or not component:
		return
	
	Telemetry["amount"] = get_amount()
	Telemetry["fill_percentage"] = get_fill_percentage()
	Telemetry["mass"] = mass
	Telemetry["pressure"] = component.get_port("port").potential

# --- Command Interface ---

func cmd_fill(args: Array):
	if args.size() > 0:
		set_amount(args[0])

func cmd_empty(args: Array):
	set_amount(0.0)

func cmd_add(args: Array):
	if args.size() > 0 and component:
		component.mass += args[0]
		_update_mass()

func cmd_remove(args: Array):
	if args.size() > 0 and component:
		component.mass = max(0.0, component.mass - args[0])
		_update_mass()
