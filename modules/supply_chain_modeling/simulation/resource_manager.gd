class_name ResourceRegistry
extends RefCounted

static var _instance: ResourceRegistry
var _resources: Dictionary = {}

static func get_instance() -> ResourceRegistry:
	if not _instance:
		_instance = ResourceRegistry.new()
	return _instance

func register_resource(resource: BaseResource) -> void:
	_resources[resource.name] = resource

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
