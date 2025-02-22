@tool
extends Node
class_name PackageManager

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

signal package_loaded(package_name: String)
signal package_loading_error(package_name: String, error: String)

var _packages: Dictionary = {}  # name -> package data
var _component_loader: ComponentLoader

func _init() -> void:
	_component_loader = ComponentLoader.new()
	add_child(_component_loader)

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
		var package_data = _component_loader.load_component_file(package_mo)
		if package_data.is_empty():
			push_error("Failed to load package.mo")
			emit_signal("package_loading_error", path.get_file(), "Failed to load package.mo")
			return false
			
		var package_name = package_data.get("name", path.get_file())
		_packages[package_name] = {
			"path": path,
			"data": package_data,
			"components": {}
		}
	
	# Then load all .mo files in the directory
	dir.list_dir_begin()
	var file_name = dir.get_next()
	while file_name != "":
		if not file_name.begins_with(".") and file_name.ends_with(".mo") and file_name != "package.mo":
			var full_path = path.path_join(file_name)
			var component_data = _component_loader.load_component_file(full_path)
			
			if not component_data.is_empty():
				var component_name = component_data.get("name", file_name.get_basename())
				var package_name = _get_package_name(path)
				
				if not _packages.has(package_name):
					_packages[package_name] = {
						"path": path,
						"data": {},
						"components": {}
					}
				
				_packages[package_name].components[component_name] = {
					"path": full_path,
					"data": component_data
				}
				
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
		var package_data = _component_loader.get_component(package_mo)
		if not package_data.is_empty():
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

func resolve_type(type_name: String, current_package: String) -> Dictionary:
	# First check in current package
	if _packages.has(current_package):
		var components = _packages[current_package].components
		if components.has(type_name):
			return components[type_name].data
	
	# Then check in all packages
	for package_name in _packages:
		var components = _packages[package_name].components
		if components.has(type_name):
			return components[type_name].data
	
	return {}
