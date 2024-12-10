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
@export var current_amount: float = 100.0  # Current amount of resource available
@export var max_amount: float = 100.0  # Maximum storage capacity

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
	update_display()

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
	var amount_to_remove = min(amount, current_amount)
	current_amount -= amount_to_remove
	update_display()
	return amount_to_remove

func add_resource(amount: float) -> float:
	var space_available = max_amount - current_amount
	var amount_to_add = min(amount, space_available)
	current_amount += amount_to_add
	update_display()
	return amount_to_add

func update_display() -> void:
	# Update the progress bar
	var progress = $Properties/ProgressBar
	if progress:
		progress.max_value = max_amount
		progress.value = current_amount
		
	# Update the amount label
	var amount_label = $Properties/Amount
	if amount_label:
		amount_label.text = "Amount: %.2f / %.2f units" % [current_amount, max_amount]
