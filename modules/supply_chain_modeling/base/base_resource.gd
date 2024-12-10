extends GraphNode

class_name BaseResource

# Basic resource properties as defined in SPECIFICATION.md
var resource_id: String
@export var description: String
@export var resource_type: String  # product, service, or custom
@export var mass: float = 0.0
@export var volume: float = 0.0
var custom_properties: Dictionary = {}
var metadata: Dictionary = {}

func _init():
	# Set up basic GraphNode properties
	mouse_filter = MOUSE_FILTER_PASS
	resizable = true
	
	# Configure the output slot (right side)
	set_slot_enabled_right(0, true)
	set_slot_type_right(0, 0)
	set_slot_color_right(0, Color.WHITE)

func _ready():
	# Disable left slot (resources are sources)
	set_slot_enabled_left(0, false)
	
	# Set up basic appearance
	size = Vector2(150, 80)  # Default size
	
func set_resource_properties(id: String, desc: String, type: String):
	resource_id = id
	description = desc
	resource_type = type
	title = "Resource: " + id

func get_resource_data() -> Dictionary:
	return {
		"id": resource_id,
		"description": description,
		"type": resource_type,
		"mass": mass,
		"volume": volume,
		"custom_properties": custom_properties,
		"metadata": metadata
	}

func load_resource_data(data: Dictionary) -> void:
	if "id" in data:
		resource_id = data.id
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

func remove_resource(amount: float) -> float:
	# Implement logic to remove the specified amount of resource
	# For example, you might want to track the current amount of resource available
	# and return the amount that can actually be removed.
	var amount_to_remove = min(amount, mass)  # Assuming mass represents the available amount
	mass -= amount_to_remove
	return amount_to_remove
