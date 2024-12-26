class_name StorageFacility
extends BaseFacility

@export var capacity: float = 100.0  # Maximum storage capacity
@export var current_amount: float = 0.0  # Current amount stored
@export var stored_resource_type: String = ""

func _init() -> void:
	pass

func available_space() -> float:
	return capacity - current_amount

func add_resource(amount: float) -> float:
	var space_available = available_space()
	var amount_to_add = min(amount, space_available)
	current_amount += amount_to_add
	
	return amount_to_add

func remove_resource(amount: float) -> float:
	var amount_to_remove = min(amount, current_amount)
	current_amount -= amount_to_remove
	return amount_to_remove 

func get_connected_outputs() -> Array:
	var outputs = []
	var simulation = get_parent()
	if simulation:
		for connection in simulation.connections:
			if connection["from_node"] == name:
				var target = simulation.get_node(connection["to_node"])
				if target:
					outputs.append(target)
	return outputs

func save_state() -> Dictionary:
	var state = super.save_state()
	state["capacity"] = capacity
	state["current_amount"] = current_amount
	state["stored_resource_type"] = stored_resource_type
	
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)

func can_store_resource(resource_type: String) -> bool:
	return stored_resource_type == "" or stored_resource_type == resource_type

func set_resource_type(type: String) -> void:
	if current_amount == 0:
		stored_resource_type = type
