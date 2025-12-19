extends Node

## Global registry for all resource types
##
## Allows dynamic registration and lookup of resources.
## Resources can be registered from code, .tres files, or JSON.

var resources: Dictionary = {}  # resource_id -> LCResourceDefinition

signal resource_registered(definition: LCResourceDefinition)

func _ready():
	print("ResourceRegistry: Initializing")
	_load_builtin_resources()
	_load_user_resources()
	print("ResourceRegistry: Loaded ", resources.size(), " resources")

## Register a resource definition
func register_resource(definition: LCResourceDefinition) -> bool:
	if not definition or definition.resource_id.is_empty():
		push_error("ResourceRegistry: Invalid resource definition")
		return false
	
	if resources.has(definition.resource_id):
		push_warning("ResourceRegistry: Resource already registered: " + definition.resource_id)
		return false
	
	resources[definition.resource_id] = definition
	resource_registered.emit(definition)
	print("ResourceRegistry: Registered resource: ", definition.display_name, " (", definition.resource_id, ")")
	return true

## Register resource from dictionary (for JSON loading)
func register_resource_from_dict(data: Dictionary, source: String = "unknown") -> bool:
	var definition = LCResourceDefinition.new()
	definition.resource_id = data.get("id", "")
	definition.display_name = data.get("name", "")
	definition.description = data.get("description", "")
	definition.category = data.get("category", "generic")
	definition.density = data.get("density", 1.0)
	definition.specific_heat = data.get("specific_heat", 1000.0)
	definition.phase_at_stp = data.get("phase", "solid")
	definition.can_flow = data.get("can_flow", true)
	definition.requires_pressure = data.get("requires_pressure", false)
	definition.requires_temperature_control = data.get("requires_temperature_control", false)
	definition.flow_rate_multiplier = data.get("flow_rate_multiplier", 1.0)
	definition.color = Color(data.get("color", "#FFFFFF"))
	definition.tags = PackedStringArray(data.get("tags", []))
	definition.custom_properties = data.get("custom", {})
	
	return register_resource(definition)

## Get resource definition by ID
func get_resource(resource_id: String) -> LCResourceDefinition:
	return resources.get(resource_id)

## Get all registered resources
func get_all_resources() -> Array[LCResourceDefinition]:
	var result: Array[LCResourceDefinition] = []
	result.assign(resources.values())
	return result

## Get resources by category
func get_resources_by_category(category: String) -> Array[LCResourceDefinition]:
	var result: Array[LCResourceDefinition] = []
	for res in resources.values():
		if res.is_category(category):
			result.append(res)
	return result

## Get resources by tag
func get_resources_by_tag(tag: String) -> Array[LCResourceDefinition]:
	var result: Array[LCResourceDefinition] = []
	for res in resources.values():
		if res.has_tag(tag):
			result.append(res)
	return result

## Check if resource exists
func has_resource(resource_id: String) -> bool:
	return resources.has(resource_id)

# Load built-in resources from res://core/resources/definitions/
func _load_builtin_resources():
	var dir = DirAccess.open("res://core/resources/definitions/")
	if not dir:
		print("ResourceRegistry: No built-in resources directory found")
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	while file_name != "":
		if file_name.ends_with(".tres") or file_name.ends_with(".res"):
			var res = load("res://core/resources/definitions/" + file_name)
			if res is LCResourceDefinition:
				register_resource(res)
		file_name = dir.get_next()

# Load user-defined resources from user://resources/
func _load_user_resources():
	var dir = DirAccess.open("user://resources/")
	if not dir:
		# Create directory if it doesn't exist
		DirAccess.make_dir_absolute("user://resources/")
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	while file_name != "":
		if file_name.ends_with(".json"):
			_load_resource_from_json("user://resources/" + file_name)
		file_name = dir.get_next()

func _load_resource_from_json(path: String):
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		return
	
	var json_text = file.get_as_text()
	var json = JSON.parse_string(json_text)
	
	if json and json is Dictionary:
		register_resource_from_dict(json, path)
	else:
		push_error("ResourceRegistry: Failed to parse JSON: " + path)
