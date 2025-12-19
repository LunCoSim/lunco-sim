class_name LCResourceDefinition
extends Resource

## Defines a type of resource that can flow through the system
##
## Resources are dynamically registered and can be added via plugins/mods.
## Each resource has physical properties, flow characteristics, and metadata.

@export var resource_id: String = ""  ## Unique identifier (e.g., "oxygen", "regolith")
@export var display_name: String = ""  ## Human-readable name
@export_multiline var description: String = ""
@export var category: String = "generic"  ## "gas", "liquid", "solid", "energy"
@export var unit: String = "kg"  ## Display unit (e.g., "kg", "L", "kWh")

@export_group("Physical Properties")
@export var density: float = 1.0  ## kg/m³ or kg/L
@export var specific_heat: float = 1000.0  ## J/(kg·K)
@export var phase_at_stp: String = "solid"  ## "solid", "liquid", "gas"

@export_group("Flow Properties")
@export var can_flow: bool = true
@export var requires_pressure: bool = false
@export var requires_temperature_control: bool = false
@export var flow_rate_multiplier: float = 1.0  ## Affects how fast it flows

@export_group("Visual")
@export var color: Color = Color.WHITE
@export var icon: Texture2D = null

@export_group("Metadata")
@export var tags: PackedStringArray = []  ## ["breathable", "fuel", "oxidizer", etc.]

## Custom properties for extensibility (used by mods/plugins)
var custom_properties: Dictionary = {}

func has_tag(tag: String) -> bool:
	return tag in tags

func is_category(cat: String) -> bool:
	return category == cat or category.begins_with(cat + ".")
