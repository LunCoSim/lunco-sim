extends UISimulationNode

class_name BaseFacility

# Basic facility properties as defined in SPECIFICATION.md
@export var facility_id: String
@export var description: String
@export var facility_type: String  # producer, consumer, storage, or custom
@export var efficiency: float = 1.0
@export var status: String = "Running"
var custom_properties: Dictionary = {}
var metadata: Dictionary = {}

func _init():
	# Set up basic GraphNode properties
	mouse_filter = MOUSE_FILTER_PASS
	resizable = true

func _ready():
	super._ready()
	update_status_display()

func update_from_simulation() -> void:
	super.update_from_simulation()
	if simulation_node:
		$Parameters/Status.text = simulation_node.properties.status
		$Parameters/Efficiency.text = "Efficiency: " + str(simulation_node.properties.efficiency)
		
func set_facility_properties(id: String, desc: String, type: String):
	facility_id = id
	description = desc
	facility_type = type
	title = "Facility: " + id

func update_status_display() -> void:
	# Virtual method to be implemented by child classes
	pass

func set_status(new_status: String) -> void:
	status = new_status
	update_status_display()

func get_facility_data() -> Dictionary:
	return {
		"id": facility_id,
		"description": description,
		"type": facility_type,
		"efficiency": efficiency,
		"status": status,
		"custom_properties": custom_properties,
		"metadata": metadata
	}

func load_facility_data(data: Dictionary) -> void:
	if "id" in data:
		facility_id = data.id
	if "description" in data:
		description = data.description
	if "type" in data:
		facility_type = data.type
	if "efficiency" in data:
		efficiency = data.efficiency
	if "status" in data:
		status = data.status
	if "custom_properties" in data:
		custom_properties = data.custom_properties
	if "metadata" in data:
		metadata = data.metadata 
