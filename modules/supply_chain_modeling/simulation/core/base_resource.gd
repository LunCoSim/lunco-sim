class_name BaseResource
extends SimulationNode

@export var current_amount: float = 2000.0
@export var max_amount: float = 2000.0
@export var resource_type: String  # product, service, or custom
@export var mass: float = 0.0
@export var volume: float = 0.0
@export var unit: String = "units"
@export var color: Color = Color.WHITE

var custom_properties: Dictionary = {}
var metadata: Dictionary = {}

# Default properties
var default_resource_type: String = ""
var default_mass: float = 1.0
var default_volume: float = 1.0
var default_current_amount: float = 0.0
var default_max_amount: float = 1000.0
var default_unit: String = "units"
var default_color: Color = Color.WHITE

# Function to set properties
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

# Save/load state
func save_state() -> Dictionary:
	var state = super.save_state()
	
	state["type"] = resource_type
	state["mass"] = mass
	state["volume"] = volume
	state["custom_properties"] = custom_properties
	state["metadata"] = metadata
	state["current_amount"] = current_amount
	state["max_amount"] = max_amount
	state["unit"] = unit
	# Save color as RGBA components
	state["color"] = {
		"r": color.r,
		"g": color.g,
		"b": color.b,
		"a": color.a
	}
	return state

func load_state(state: Dictionary) -> void:
	if state:
		resource_type = state.get("type", default_resource_type)
		mass = state.get("mass", default_mass)
		volume = state.get("volume", default_volume)
		custom_properties = state.get("custom_properties", {})
		metadata = state.get("metadata", {})
		current_amount = state.get("current_amount", default_current_amount)
		max_amount = state.get("max_amount", default_max_amount)
		unit = state.get("unit", default_unit)
		
		# Load color from components
		var color_data = state.get("color", null)
		if color_data is Dictionary:
			color = Color(
				color_data.get("r", 1.0),
				color_data.get("g", 1.0),
				color_data.get("b", 1.0),
				color_data.get("a", 1.0)
			)
		elif color_data is String:
			# Handle legacy string format
			color = Color(color_data)
		else:
			color = default_color
	
