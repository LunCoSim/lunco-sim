@tool
extends Node
class_name PackageManager

signal package_loaded(package_name: String)
signal loading_error(package_name: String, error: String)

var _packages: Dictionary = {}
var _loading_queue: Array = []
var _is_loading: bool = false

func _init() -> void:
	_packages = {}
	_loading_queue = []

func load_package(path: String) -> bool:
	print("Loading package from: ", path)
	
	# Check if package.mo exists
	var package_mo = path.path_join("package.mo")
	if not FileAccess.file_exists(package_mo):
		push_error("No package.mo found at: " + package_mo)
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
	
	# Load subpackages
	_load_subpackages(path)
	
	emit_signal("package_loaded", package_data.name)
	return true

func has_package(package_name: String) -> bool:
	return _packages.has(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
	return _packages.get(package_name, {})

func get_loaded_packages() -> Array:
	return _packages.keys()

func _parse_package_mo(content: String, path: String) -> Dictionary:
	var package_data = {}
	
	# Create RegEx patterns
	var name_regex = RegEx.new()
	name_regex.compile("package\\s+(\\w+)")
	var doc_regex = RegEx.new()
	doc_regex.compile('Documentation\\(info="([^"]+)"\\)')
	
	# Extract package name
	var name_match = name_regex.search(content)
	if name_match and name_match.get_string_count() > 1:
		package_data["name"] = name_match.get_string(1)
	else:
		package_data["name"] = path.get_file()
	
	package_data["path"] = path
	package_data["type"] = "package"
	
	# Extract documentation if available
	var doc_match = doc_regex.search(content)
	if doc_match and doc_match.get_string_count() > 1:
		package_data["description"] = doc_match.get_string(1).strip_edges()
	
	return package_data

func _load_subpackages(path: String) -> void:
	var dir = DirAccess.open(path)
	if not dir:
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		if not file_name.begins_with("."):
			var full_path = path.path_join(file_name)
			
			if dir.current_is_dir():
				var subpackage_mo = full_path.path_join("package.mo")
				if FileAccess.file_exists(subpackage_mo):
					load_package(full_path)
		
		file_name = dir.get_next()
	
	dir.list_dir_end() 
