class_name BaseResource
extends SimulationNode

@export var current_amount: float = 2000.0
@export var max_amount: float = 2000.0
@export var description: String
@export var resource_type: String  # product, service, or custom
@export var mass: float = 0.0
@export var volume: float = 0.0
var custom_properties: Dictionary = {}
var metadata: Dictionary = {}


func set_properties(desc: String, type: String, init_mass: float, init_volume: float):
	description = desc
	resource_type = type
	mass = init_mass
	volume = init_volume

func remove_resource(amount: float) -> float:
	var available = min(amount, current_amount)
	current_amount -= available
	return available

func add_resource(amount: float) -> float:
	var space_available = max_amount - current_amount
	var amount_to_add = min(amount, space_available)
	current_amount += amount_to_add
	return amount_to_add

func save_state() -> Dictionary:
	return {
		"description": description,
		"type": resource_type,
		"mass": mass,
		"volume": volume,
		"custom_properties": custom_properties,
		"metadata": metadata,
		"current_amount": current_amount,
		"max_amount": max_amount
	}

func load_state(data: Dictionary) -> void:
	if "description" in data:
		description = data.description
	if "type" in data:
		resource_type = data.type
	if "mass" in data:
		mass = data.mass
	if "volume" in data:
		volume = data.volume
	if "custom_properties" in data:
		custom_properties = data.custom_properties
	if "metadata" in data:
		metadata = data.metadata
	if "current_amount" in data:
		current_amount = data.current_amount
	if "max_amount" in data:
		max_amount = data.max_amount
