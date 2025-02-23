@tool
extends BaseLoader
class_name PackageLoader

signal package_loaded(package_name: String)
signal package_loading_error(package_name: String, error: String)

var _packages: Dictionary = {}  # name -> package data
var _msl_path: String = ""

func set_msl_path(path: String) -> void:
    _msl_path = path

func get_msl_path() -> String:
    return _msl_path

func has_msl() -> bool:
    return not _msl_path.is_empty() and DirAccess.dir_exists_absolute(_msl_path)

func load_msl() -> bool:
    if not has_msl():
        push_error("MSL path not set or invalid")
        return false
    return load_package(_msl_path)

func load_package(path: String) -> bool:
    print("Loading package from path: ", path)
    
    # Check if directory exists
    var dir = DirAccess.open(path)
    if not dir:
        push_error("Failed to open directory: " + path)
        emit_signal("package_loading_error", path.get_file(), "Directory not found")
        return false
    
    # First load package.mo if it exists
    var package_mo = path.path_join("package.mo")
    if FileAccess.file_exists(package_mo):
        var package_data = load_file(package_mo)
        if not validate_model_data(package_data):
            push_error("Failed to load package.mo")
            emit_signal("package_loading_error", path.get_file(), "Failed to load package.mo")
            return false
            
        var package_name = package_data.get("name", path.get_file())
        _packages[package_name] = {
            "path": path,
            "data": package_data,
            "components": {},
            "uses": []  # Track package dependencies
        }
    
    # Then load all .mo files in the directory
    dir.list_dir_begin()
    var file_name = dir.get_next()
    while file_name != "":
        if not file_name.begins_with(".") and file_name.ends_with(".mo") and file_name != "package.mo":
            var full_path = path.path_join(file_name)
            var component_data = load_file(full_path)
            
            if validate_model_data(component_data):
                var component_name = component_data.get("name", file_name.get_basename())
                var package_name = _get_package_name(path)
                
                if not _packages.has(package_name):
                    _packages[package_name] = {
                        "path": path,
                        "data": {},
                        "components": {},
                        "uses": []
                    }
                
                _packages[package_name].components[component_name] = {
                    "path": full_path,
                    "data": component_data
                }
                
                # Track package dependencies from imports/uses
                var uses = component_data.get("uses", [])
                for used_package in uses:
                    if not used_package in _packages[package_name].uses:
                        _packages[package_name].uses.append(used_package)
                
                print("Added component ", component_name, " to package ", package_name)
        
        file_name = dir.get_next()
    
    dir.list_dir_end()
    
    # Load subdirectories recursively
    dir.list_dir_begin()
    file_name = dir.get_next()
    while file_name != "":
        if not file_name.begins_with(".") and dir.current_is_dir():
            var subdir_path = path.path_join(file_name)
            load_package(subdir_path)
        file_name = dir.get_next()
    dir.list_dir_end()
    
    emit_signal("package_loaded", path.get_file())
    return true

func _get_package_name(path: String) -> String:
    var package_mo = path.path_join("package.mo")
    if FileAccess.file_exists(package_mo):
        var package_data = load_file(package_mo)
        if validate_model_data(package_data):
            return package_data.get("name", path.get_file())
    return path.get_file()

func has_package(package_name: String) -> bool:
    return _packages.has(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
    if not _packages.has(package_name):
        return {}
    return _packages[package_name].data

func get_loaded_packages() -> Array:
    return _packages.keys()

func get_package_components(package_name: String) -> Dictionary:
    if not _packages.has(package_name):
        return {}
    return _packages[package_name].components

func get_package_dependencies(package_name: String) -> Array:
    if not _packages.has(package_name):
        return []
    return _packages[package_name].uses

func resolve_type(type_name: String, current_package: String) -> Dictionary:
    # First check in current package
    if _packages.has(current_package):
        var components = _packages[current_package].components
        if components.has(type_name):
            return components[type_name].data
    
    # Then check in used packages
    if _packages.has(current_package):
        for used_package in _packages[current_package].uses:
            if _packages.has(used_package):
                var components = _packages[used_package].components
                if components.has(type_name):
                    return components[type_name].data
    
    # Finally check in MSL if available
    if has_msl():
        var msl_components = _packages.get("Modelica", {}).get("components", {})
        if msl_components.has(type_name):
            return msl_components[type_name].data
    
    return {} 