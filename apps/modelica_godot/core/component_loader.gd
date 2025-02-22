@tool
extends Node
class_name ComponentLoader

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

var _parser: MOParser
var _components: Dictionary = {}  # path -> model_data
var _connectors: Dictionary = {}  # name -> connector_data

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
        # Check if this is a connector definition
        if model_data.get("type", "") == "connector":
            _process_connector(model_data)
        
        _components[path] = model_data
        print("Loaded component: ", model_data.get("name", ""), " of type: ", model_data.get("type", ""))
        
    return model_data

func _process_connector(connector_data: Dictionary) -> void:
    var name = connector_data.get("name", "")
    if name.is_empty():
        push_error("Connector has no name")
        return
        
    var processed_data = {
        "name": name,
        "type": "connector",
        "variables": {},
        "flow_variables": [],  # Track flow variables separately
        "potential_variables": []  # Track potential variables
    }
    
    # Process variables
    for var_data in connector_data.get("variables", []):
        var var_name = var_data.get("name", "")
        if var_name.is_empty():
            continue
            
        processed_data.variables[var_name] = var_data
        
        # Track flow and potential variables
        if var_data.get("flow", false):
            processed_data.flow_variables.append(var_name)
        else:
            processed_data.potential_variables.append(var_name)
    
    _connectors[name] = processed_data
    print("Processed connector: ", name, " with flow variables: ", processed_data.flow_variables)

func get_component(path: String) -> Dictionary:
    return _components.get(path, {})

func has_component(name: String) -> bool:
    # First check connectors
    if _connectors.has(name):
        return true
        
    # Then check components
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return true
    return false

func get_component_by_name(name: String) -> Dictionary:
    # First check connectors
    if _connectors.has(name):
        return _connectors[name]
        
    # Then check components
    for path in _components:
        var model = _components[path]
        if model.get("name", "") == name:
            return model
    return {}

func get_connector_info(name: String) -> Dictionary:
    return _connectors.get(name, {})

func is_flow_variable(connector_name: String, variable_name: String) -> bool:
    var connector = _connectors.get(connector_name, {})
    return variable_name in connector.get("flow_variables", []) 