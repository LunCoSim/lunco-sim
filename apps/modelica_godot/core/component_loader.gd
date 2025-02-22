@tool
extends Node
class_name ComponentLoader

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

var _parser: MOParser
var _components: Dictionary = {}  # path -> model_data

func _init() -> void:
    _parser = MOParser.new()

func load_component_file(path: String) -> Dictionary:
    print("Loading component from: ", path)
    
    # Check if already loaded
    if _components.has(path):
        return _components[path]
    
    # Load and parse file
    var file = FileAccess.open(path, FileAccess.READ)
    if not file:
        push_error("Failed to open file: " + path)
        return {}
    
    var content = file.get_as_text()
    var model_data = _parser.parse_text(content)
    
    if not model_data.is_empty():
        _components[path] = model_data
        
    return model_data

func get_component(path: String) -> Dictionary:
    return _components.get(path, {})

func has_component(name: String) -> bool:
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return true
    return false

func get_component_by_name(name: String) -> Dictionary:
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return model
    return {} 