class_name StorageFacility
extends SolverSimulationNode

const SolverDomain = preload("res://core/systems/solver/solver_domain.gd")

@export var capacity: float = 100.0  # Maximum storage capacity (kg)
@export var current_amount: float = 0.0  # Current amount stored (kg)
@export var stored_resource_type: String = ""  # Name of the resource type

var _resource: LCResourceDefinition = null

func _ready() -> void:
	_update_resource()

func _update_resource() -> void:
	if stored_resource_type != "":
		# Use connect to global LCResourceRegistry autoload safely
		var registry = get_node_or_null("/root/LCResourceRegistry")
		if registry:
			if registry.has_resource(stored_resource_type):
				_resource = registry.get_resource(stored_resource_type)
			else:
				push_warning("StorageFacility: Resource not found: " + stored_resource_type)
		else:
			push_warning("StorageFacility: ResourceRegistry not found")

func set_capacity(value: float) -> void:
	capacity = value
	if ports.has("fluid_port"):
		ports["fluid_port"].set_capacitance(max(capacity, 0.1))

func set_current_amount(value: float) -> void:
	current_amount = value
	if ports.has("fluid_port"):
		ports["fluid_port"].flow_accumulation = current_amount
		# Also update potential if capacity is known
		if ports["fluid_port"].capacitance > 0:
			ports["fluid_port"].potential = value / ports["fluid_port"].capacitance

func set_resource_type(type: String) -> void:
	# ... existing logic ...
	# Allow changing type freely for now to avoid editor frustration
	# OR: only if amount is default 100?
	# Let's just allow it and print a warning if we are mixing (which we overwrite anyway)
	
	if stored_resource_type != type:
		print("StorageFacility: Changing type from '%s' to '%s' (Amount: %.2f)" % [stored_resource_type, type, current_amount])
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
	# Determine domain based on resource type
	var domain = SolverDomain.LIQUID
	if stored_resource_type in ["oxygen", "hydrogen", "methane"]:
		domain = SolverDomain.GAS
	elif stored_resource_type == "regolith":
		domain = SolverDomain.SOLID
		
	# Create storage node with capacitance
	var port = solver_graph.add_node(0.0, false, domain)
	port.resource_type = stored_resource_type
	port.set_capacitance(max(capacity, 0.1))  # Set proper capacitance from the start
	port.flow_accumulation = current_amount
	# Calculate initial potential from mass and capacitance
	if port.capacitance > 0:
		port.potential = port.flow_accumulation / port.capacitance
	ports["fluid_port"] = port
	print("StorageFacility [%s]: Port created. Type='%s', Amount=%.2f, Cap=%.2f" % [name, stored_resource_type, current_amount, capacity])

## Update solver parameters from component state
func update_solver_state():
	if not ports.has("fluid_port"):
		return
		
	var port: LCSolverNode = ports["fluid_port"]
	
	# Debug check for zero type
	if stored_resource_type == "" and current_amount > 0:
		print("StorageFacility [%s]: WARNING - Amount > 0 but Type is EMPTY!" % name)
	
	# Update capacitance based on capacity
	# Using simple linear model: Pressure = Mass / Capacitance
	# For a tank, we want Pressure to represent "fill level" in some way
	# Let's use: Capacitance = Capacity (so Pressure = Mass / Capacity = fill ratio)
	port.set_capacitance(max(capacity, 0.1))  # Avoid division by zero
	
	# Ensure resource type is synced
	if port.resource_type != stored_resource_type:
		print("StorageFacility [%s]: Syncing solver type '%s' -> '%s'" % [name, port.resource_type, stored_resource_type])
		port.resource_type = stored_resource_type
	
	# Sync user changes to solver (if mass was updated externally)
	# This assumes current_amount is the source of truth if it deviates significantly
	# from what the solver last reported (e.g. user edit in inspector).
	# However, since update_from_solver overwrites current_amount every frame,
	# we can just push current_amount to flow_accumulation here to support
	# external edits and initialization.
	port.flow_accumulation = current_amount

## Update component state from solver results
func update_from_solver():
	if not ports.has("fluid_port"):
		return
		
	var port: LCSolverNode = ports["fluid_port"]
	current_amount = port.flow_accumulation
	
	# Auto-classify resource type if generic and receiving flow
	if stored_resource_type == "" and current_amount > 0.0001:
		for edge in port.edges:
			var neighbor = edge.node_a if edge.node_b == port else edge.node_b
			if neighbor.resource_type != "" and neighbor.resource_type != "fluid":
				print("StorageFacility [%s]: Auto-detected resource type '%s' from neighbor" % [name, neighbor.resource_type])
				set_resource_type(neighbor.resource_type)
				break
	


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
