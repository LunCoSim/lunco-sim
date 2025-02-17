extends Node

const MOParser = preload("res://apps/modelica_godot/core/mo_parser.gd")

signal package_loaded(package_name: String)
signal loading_error(package_name: String, error: String)

var _packages: Dictionary = {}  # full_qualified_name -> package_data
var _components: Dictionary = {}  # full_path -> component_data
var _package_hierarchy: Dictionary = {}  # package_path -> parent_package

func load_package(path: String) -> bool:
	print("\n=== Loading Package ===")
	print("Path: ", path)
	
	# Check if package.mo exists
	var package_mo = path.path_join("package.mo")
	if not FileAccess.file_exists(package_mo):
		push_error("No package.mo found at: " + package_mo)
		emit_signal("loading_error", path.get_file(), "No package.mo found")
		return false
	
	print("Found package.mo at: ", package_mo)
	
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
	
	# Build full qualified name
	var full_name = package_data["name"]
	if package_data.has("within"):
		full_name = package_data["within"] + "." + full_name
	
	print("Package Info:")
	print("- Name: ", package_data["name"])
	print("- Full Name: ", full_name)
	print("- Within: ", package_data.get("within", ""))
	
	# Store package data
	_packages[full_name] = package_data
	print("Stored package: ", full_name)
	
	# Store in hierarchy
	_package_hierarchy[path] = package_data.get("within", "")
	
	# Load all .mo files in the package directory
	print("\n=== Loading Components ===")
	_load_package_components(path, full_name)
	
	emit_signal("package_loaded", full_name)
	return true

func _parse_package_mo(content: String, path: String) -> Dictionary:
	print("\nParsing package.mo")
	
	# Use MOParser to parse the package file
	var parser = MOParser.new()
	var result = parser.parse_text(content)
	
	if result.is_empty() or result.type != "package":
		push_error("Invalid package.mo file at: " + path)
		return {}
	
	var package_data = {
		"name": result.name,
		"within": result.get("within", ""),
		"path": path,
		"type": "package",
		"components": result.get("components", [])
	}
	
	return package_data

func _load_package_components(path: String, parent_package: String) -> void:
	print("\nLoading components from directory: ", path)
	print("Parent package: ", parent_package)
	
	var dir = DirAccess.open(path)
	if not dir:
		push_error("Failed to open directory: " + path)
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while not file_name.is_empty():
		if not file_name.begins_with("."):  # Skip hidden files
			var full_path = path.path_join(file_name)
			
			if dir.current_is_dir():
				print("\nEntering subdirectory: ", file_name)
				# First check if it's a package
				var package_mo = full_path.path_join("package.mo")
				if FileAccess.file_exists(package_mo):
					print("Found package.mo in subdirectory")
					load_package(full_path)
				else:
					# If not a package, just load components
					_load_package_components(full_path, parent_package)
			elif file_name.ends_with(".mo") and file_name != "package.mo":
				print("\nLoading component: ", file_name)
				_load_component(full_path, parent_package)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()

func _load_component(path: String, parent_package: String) -> void:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		push_error("Failed to open component file: " + path)
		return
	
	var content = file.get_as_text()
	print("Parsing component: ", path.get_file())
	var parser = MOParser.new()
	var result = parser.parse_text(content)
	
	if not result.is_empty():
		var component_data = {
			"name": result.name,
			"type": result.type,
			"within": result.get("within", parent_package),
			"path": path,
			"components": result.get("components", [])
		}
		
		print("Component Info:")
		print("- Type: ", component_data.type)
		print("- Name: ", component_data.name)
		print("- Within: ", component_data.within)
		
		_components[path] = component_data
		print("Component loaded successfully")
	else:
		push_error("Failed to parse component: " + path)

func has_package(package_name: String) -> bool:
	return _packages.has(package_name)

func get_package_metadata(package_name: String) -> Dictionary:
	return _packages.get(package_name, {})

func get_loaded_packages() -> Array:
	return _packages.keys()

func get_component(path: String) -> Dictionary:
	return _components.get(path, {})
