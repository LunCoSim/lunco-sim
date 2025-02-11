class_name BaseFacility
extends SimulationNode

# Basic facility properties as defined in SPECIFICATION.md
@export var facility_type: String  # producer, consumer, storage, or custom
@export var efficiency: float = 1.0
@export var status: String = "Running"

var custom_properties: Dictionary = {}
var metadata: Dictionary = {}

# Default properties
var default_facility_type: String = ""
var default_efficiency: float = 1.0
var default_status: String = "Running"	

# Function to save the state of the facility
func save_state() -> Dictionary:
	var state = super.save_state()
	
	state["type"] = facility_type
	state["efficiency"] = efficiency
	state["status"] = status
	state["custom_properties"] = custom_properties
	state["metadata"] = metadata

	return state

func load_state(state: Dictionary) -> void:
	if state:
		facility_type = state.get("type", "")
		efficiency = state.get("efficiency", 1.0)
		status = state.get("status", "Running")
		custom_properties = state.get("custom_properties", {})
		metadata = state.get("metadata", {})

	super.load_state(state)
