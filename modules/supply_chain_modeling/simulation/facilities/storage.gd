class_name StorageFacility
extends BaseFacility

@export var capacity: float = 100.0  # Maximum storage capacity
@export var current_amount: float = 0.0  # Current amount stored

@export var resource_type: String = ""  # Type of resource being stored TBD: should be a BaseResource

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
	
	
	return state

func load_state(state: Dictionary) -> void:
	super.load_state(state)
