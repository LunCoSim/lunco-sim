class_name StorageFacility
extends SolverSimulationNode

@export var capacity: float = 100.0  # Maximum storage capacity (kg)
@export var current_amount: float = 0.0  # Current amount stored (kg)
@export var stored_resource_type: String = ""  # Name of the resource type

var _resource: LCResourceDefinition = null

func _ready() -> void:
	_update_resource()

func _update_resource() -> void:
	if stored_resource_type != "":
		_resource = ResourceRegistry.get_resource(stored_resource_type)

func set_resource_type(type: String) -> void:
	if current_amount == 0 or type == stored_resource_type:
		stored_resource_type = type
		_update_resource()
		
		# Update solver node resource type
		if ports.has("fluid_port"):
			ports["fluid_port"].resource_type = type

func get_resource_color() -> Color:
	return _resource.color if _resource else Color.WHITE

func get_resource_unit() -> String:
	return _resource.unit if _resource else "units"

## Create a single storage port
func _create_ports():
	# Create storage node with capacitance
	var port = solver_graph.add_node(0.0, false, "Fluid")
	port.resource_type = stored_resource_type
	port.set_capacitance(1.0)  # Will be updated in update_solver_state
	port.flow_accumulation = current_amount
	ports["fluid_port"] = port

## Update solver parameters from component state
func update_solver_state():
	if not ports.has("fluid_port"):
		return
		
	var port: LCSolverNode = ports["fluid_port"]
	
	# Update capacitance based on capacity
	# Using simple linear model: Pressure = Mass / Capacitance
	# For a tank, we want Pressure to represent "fill level" in some way
	# Let's use: Capacitance = Capacity (so Pressure = Mass / Capacity = fill ratio)
	port.set_capacitance(max(capacity, 0.1))  # Avoid division by zero
	
	# Ensure resource type is synced
	port.resource_type = stored_resource_type

## Update component state from solver results
func update_from_solver():
	if not ports.has("fluid_port"):
		return
		
	var port: LCSolverNode = ports["fluid_port"]
	current_amount = port.flow_accumulation

func available_space() -> float:
	return capacity - current_amount

func save_state() -> Dictionary:
	var state = super.save_state()
	state["capacity"] = capacity
	state["current_amount"] = current_amount
	state["stored_resource_type"] = stored_resource_type
	
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
	capacity = state.get("capacity", capacity)
	current_amount = state.get("current_amount", current_amount)
	stored_resource_type = state.get("stored_resource_type", stored_resource_type)
	_update_resource()

func can_store_resource(resource_type: String) -> bool:
	return stored_resource_type == "" or stored_resource_type == resource_type

func can_connect_with(other_node: Node, from_port: int, to_port: int) -> bool:
	# If we're the source (from_node)
	if other_node is StorageFacility:
		var other_storage = other_node as StorageFacility
		
		# If other storage has no type set, it can accept our type
		if other_storage.stored_resource_type == "":
			return true
			
		# If we have no type set, we can't connect
		if stored_resource_type == "":
			return false
			
		# Check if resource types match
		return stored_resource_type == other_storage.stored_resource_type
		
	return true  # Default to true for non-storage nodes for now
