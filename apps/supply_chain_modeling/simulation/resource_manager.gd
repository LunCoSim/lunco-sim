class_name ResourceRegistry
extends RefCounted

static var _instance: ResourceRegistry
var _resources: Dictionary = {}

static func get_instance() -> ResourceRegistry:
	if not _instance:
		_instance = ResourceRegistry.new()
		_instance._initialize_default_resources()
	return _instance

func _initialize_default_resources() -> void:
	# Get all resource scripts from the resources directory
	var resource_scripts = Utils.get_script_paths("res://simulation/resources/")
	
	# Create instances of each resource
	for script in resource_scripts:
		var resource = load(script).new() as BaseResource
		
		if resource:
			register_resource(resource)
	
	# Add Electricity as it's a special case
	var electricity = BaseResource.new()
	electricity.name = "Electricity"
	electricity.description = "Electrical power for facilities"
	electricity.unit = "kW"
	electricity.resource_type = "service"
	electricity.color = Color.YELLOW
	register_resource(electricity)

func register_resource(resource: BaseResource) -> void:
	_resources[resource.name] = resource
	print(resource)

func get_resource(name: String) -> BaseResource:
	return _resources.get(name)

func get_all_resources() -> Array:
	return _resources.values()

func save_state() -> Dictionary:
	var state = {}
	for key in _resources:
		state[key] = _resources[key].save_state()
	return state

func load_state(state: Dictionary) -> void:
	_resources.clear()
	for key in state:
		var resource = BaseResource.new()
		resource.load_state(state[key])
		_resources[key] = resource 
