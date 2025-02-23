@tool
extends Node
class_name BaseLoader

const MOParser = preload("res://apps/modelica_godot/core/parser/mo_parser.gd")

var _parser: MOParser

func _init() -> void:
    _parser = MOParser.new()

func load_file(path: String) -> Dictionary:
    print("Loading file: ", path)
    var file = FileAccess.open(path, FileAccess.READ)
    if not file:
        push_error("Failed to open file: " + path)
        return {}
    
    var content = file.get_as_text()
    return _parser.parse_file(content)

func validate_model_data(model_data: Dictionary) -> bool:
    if model_data.is_empty():
        return false
    
    # Check required fields
    if not model_data.has("name"):
        push_error("Model data missing name")
        return false
    
    return true

func _find_mo_files(path: String, results: Array) -> void:
    var dir = DirAccess.open(path)
    if not dir:
        push_error("Failed to open directory: " + path)
        return
    
    dir.list_dir_begin()
    var file_name = dir.get_next()
    
    while file_name != "":
        if not file_name.begins_with("."):
            var full_path = path.path_join(file_name)
            
            if dir.current_is_dir():
                _find_mo_files(full_path, results)
            elif file_name.ends_with(".mo"):
                results.append(full_path)
        
        file_name = dir.get_next()
    
    dir.list_dir_end()

func _convert_parameter_value(param: Dictionary) -> Variant:
    var value = param.get("value", "")
    var default = param.get("default", "")
    
    # Use default if value is empty
    if value.is_empty() and not default.is_empty():
        value = default
    
    # Convert based on type
    match param.get("type", "Real"):
        "Real":
            if value.is_empty():
                return 0.0
            return float(value)
        "Integer":
            if value.is_empty():
                return 0
            return int(value)
        "Boolean":
            if value.is_empty():
                return false
            return value.to_lower() == "true"
        "String":
            if value.is_empty():
                return ""
            return value.strip_edges().trim_prefix("\"").trim_suffix("\"")
        _:
            return value

func _validate_parameter(param_info: Dictionary) -> bool:
    var value = param_info["value"]
    var type = param_info["type"]
    
    # Type validation
    match type:
        "Real":
            if not (value is float or value is int):
                return false
        "Integer":
            if not value is int:
                return false
        "Boolean":
            if not value is bool:
                return false
        "String":
            if not value is String:
                return false
    
    # Range validation for numeric types
    if type in ["Real", "Integer"]:
        var min_val = param_info.get("min")
        var max_val = param_info.get("max")
        
        if min_val != null and value < min_val:
            return false
        if max_val != null and value > max_val:
            return false
    
    return true 