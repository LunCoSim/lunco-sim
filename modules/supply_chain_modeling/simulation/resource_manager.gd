class_name ResourceManager
extends RefCounted

# Singleton instance
static var _instance: ResourceManager = null

# Resource registry
var _resource_registry: Dictionary = {}
var _resource_types: Array[String] = []

# === Singleton Access ===
static func get_instance() -> ResourceManager:
	if not _instance:
		_instance = ResourceManager.new()
	return _instance

# === Initialization ===
func _init() -> void:
	if _instance:
		push_error("ResourceManager already exists!")
		return
	_instance = self
	_load_resource_types()

# === Resource Type Management ===
func _load_resource_types() -> void:
	# Step 1: Scan the resources directory
	var resources_dir = "res://simulation/resources/"
	var dir = DirAccess.open(resources_dir)
	
	if dir:
		# Step 2: Iterate through files
		dir.list_dir_begin()
		var file_name = dir.get_next()
		
		while file_name != "":
			# Only process .gd files
			if file_name.ends_with(".gd") and not file_name.begins_with("_"):
				var resource_name = file_name.get_basename()
				_resource_types.append(resource_name)
				
				# Load the script
				var script_path = resources_dir + file_name
				var resource_script = load(script_path)
				if resource_script:
					_resource_registry[resource_name] = resource_script
			
			file_name = dir.get_next()
		
		dir.list_dir_end()

# === Resource Creation ===
func create_resource(resource_type: String) -> BaseResource:
	# Step 1: Validate resource type
	if not _resource_registry.has(resource_type):
		push_error("Invalid resource type: " + resource_type)
		return null
	
	# Step 2: Create resource instance
	var resource_script = _resource_registry[resource_type]
	var resource = resource_script.new()
	
	return resource

# === Resource Type Information ===
func get_available_resource_types() -> Array[String]:
	return _resource_types

func get_resource_script(resource_type: String) -> GDScript:
	return _resource_registry.get(resource_type)

# === Resource Validation ===
func is_valid_resource_type(resource_type: String) -> bool:
	return _resource_registry.has(resource_type) 