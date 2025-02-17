@tool
extends Node
class_name PackageManager

signal package_loaded(package_name: String)
signal loading_error(package_name: String, error: String)

var _packages: Dictionary = {}  # package_name -> package_data
var _components: Dictionary = {}  # full_path -> component_data

func load_package(path: String) -> bool:
	print("Loading package from: ", path)
	
	# Find package.mo in current or parent directories
	var package_mo = _find_package_mo(path)
	if package_mo.is_empty():
		push_error("No package.mo found for: " + path)
		emit_signal("loading_error", path.get_file(), "No package.mo found")
		return false
	
	# Read and parse package.mo
	var file = FileAccess.open(package_mo, FileAccess.READ)
	if not file:
		push_error("Failed to open package.mo at: " + package_mo)
		emit_signal("loading_error", path.get_file(), "Failed to open package.mo")
		return false
	
	var content = file.get_as_text()
	var package_data = _parse_package_mo(content, path)
	
	if package_data.is_empty():
		push_error("Failed to parse package.mo at: " + package_mo)
		emit_signal("loading_error", path.get_file(), "Failed to parse package.mo")
		return false
	
	# Store package data
	_packages[package_data.name] = package_data
	
	# Load all .mo files in the package directory
	_load_package_components(path)
	
	emit_signal("package_loaded", package_data.name)
	return true

func _find_package_mo(start_path: String) -> String:
	var current_path = start_path
	while not current_path.is_empty():
		var package_mo = current_path.path_join("package.mo")
		if FileAccess.file_exists(package_mo):
			return package_mo
		
		# Move up one directory
		var parent = current_path.get_base_dir()
		if parent == current_path:
			break
		current_path = parent
	
	return ""

func _parse_package_mo(content: String, path: String) -> Dictionary:
	var package_data = {}
	
	# Create RegEx patterns
	var name_regex = RegEx.new()
	name_regex.compile("package\\s+(\\w+)")
	var within_regex = RegEx.new()
	within_regex.compile("within\\s+([\\w\\.]+)")
	
	# Extract package name
	var name_match = name_regex.search(content)
	if name_match and name_match.get_string_count() > 1:
		package_data["name"] = name_match.get_string(1)
	else:
		package_data["name"] = path.get_file()
	
	# Extract within clause if present
	var within_match = within_regex.search(content)
	if within_match and within_match.get_string_count() > 1:
		package_data["within"] = within_match.get_string(1)
	
	package_data["path"] = path
	package_data["type"] = "package"
	
	return package_data

func _load_package_components(path: String) -> void:
	var dir = DirAccess.open(path)
	if not dir:
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		if not file_name.begins_with(".") and file_name.ends_with(".mo") and file_name != "package.mo":
			var full_path = path.path_join(file_name)
			_load_component(full_path)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()

func _load_component(path: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		return
	
	var content = file.get_as_text()
	var parser = MOParser.new()
	var component_data = parser.parse_text(content)
	
	if not component_data.is_empty():
		component_data["path"] = path
		_components[path] = component_data

func has_package(package_name: String) -> bool:
	return _packages.has(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
	return _packages.get(package_name, {})

func get_component(path: String) -> Dictionary:
	return _components.get(path, {})

func get_loaded_packages() -> Array:
	return _packages.keys()

func clear() -> void:
	_packages.clear()
	_components.clear()
