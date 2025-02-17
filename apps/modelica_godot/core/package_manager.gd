@tool
extends Node
class_name PackageManager

signal package_loaded(package_name: String)
signal loading_error(package_name: String, error: String)

var _packages: Dictionary = {}  # full_qualified_name -> package_data
var _components: Dictionary = {}  # full_path -> component_data
var _package_hierarchy: Dictionary = {}  # package_path -> parent_package
var _initialized: bool = false

func _ready() -> void:
	initialize()

func initialize() -> void:
	if _initialized:
		return
		
	print("\n=== Initializing Package Manager ===")
	# Load the root package
	var root_path = "res://apps/modelica_godot/components"
	if load_package(root_path):
		print("Root package loaded successfully")
		_initialized = true
	else:
		push_error("Failed to load root package")

func load_package(path: String) -> bool:
	print("\n=== Loading Package ===")
	print("Path: ", path)
	
	# Find package.mo in current or parent directories
	var package_mo = _find_package_mo(path)
	if package_mo.is_empty():
		push_error("No package.mo found for: " + path)
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
	
	print("\n=== Package Loading Summary ===")
	print("Total components loaded: ", _components.size())
	print("Available components:")
	for comp_path in _components:
		var comp = _components[comp_path]
		print("- ", comp.get("name", "Unknown"), " (", comp.get("type", "unknown"), ")")
		print("  Within: ", comp.get("within", ""))
	
	print("\nPackage hierarchy:")
	for pkg_path in _package_hierarchy:
		print("- ", pkg_path, " -> ", _package_hierarchy[pkg_path])
	
	print("\nLoaded packages:")
	for pkg_name in _packages:
		print("- ", pkg_name)
	
	emit_signal("package_loaded", full_name)
	return true

func _find_package_mo(start_path: String) -> String:
	print("\nSearching for package.mo starting from: ", start_path)
	var current_path = start_path
	while not current_path.is_empty():
		var package_mo = current_path.path_join("package.mo")
		print("Checking: ", package_mo)
		if FileAccess.file_exists(package_mo):
			print("Found package.mo!")
			return package_mo
		
		# Move up one directory
		var parent = current_path.get_base_dir()
		if parent == current_path:
			break
		current_path = parent
	
	print("No package.mo found in path hierarchy")
	return ""

func _parse_package_mo(content: String, path: String) -> Dictionary:
	print("\nParsing package.mo")
	var package_data = {}
	
	# Create RegEx patterns
	var name_regex = RegEx.new()
	name_regex.compile("package\\s+(\\w+)")
	var within_regex = RegEx.new()
	within_regex.compile("within\\s+([\\w\\.]+)")
	
	# Extract package name
	var name_match = name_regex.search(content)
	if name_match:
		package_data["name"] = name_match.get_string(1)
		print("Found package name: ", package_data["name"])
	else:
		package_data["name"] = path.get_file()
		print("Using directory name as package name: ", package_data["name"])
	
	# Extract within clause if present
	var within_match = within_regex.search(content)
	if within_match:
		package_data["within"] = within_match.get_string(1)
		print("Found within clause: ", package_data["within"])
	
	package_data["path"] = path
	package_data["type"] = "package"
	
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
	
	while file_name != "":
		if not file_name.begins_with("."):
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
	var component_data = parser.parse_text(content)
	
	if not component_data.is_empty():
		print("Component Info:")
		print("- Type: ", component_data.get("type", "unknown"))
		print("- Name: ", component_data.get("name", "unknown"))
		
		# If no within clause, use parent package
		if not component_data.has("within") or component_data["within"].is_empty():
			component_data["within"] = parent_package
			print("Setting within to parent package: ", parent_package)
		
		print("- Within: ", component_data.get("within", ""))
		
		if component_data.has("components"):
			print("- Used Components:")
			for comp in component_data.get("components", []):
				print("  * ", comp.get("type", "unknown"), " ", comp.get("name", "unknown"))
		
		component_data["path"] = path
		_components[path] = component_data
		print("Component loaded successfully")
	else:
		push_error("Failed to parse component: " + path)

func has_package(package_name: String) -> bool:
	if not _initialized:
		initialize()
		
	print("\nChecking for package: ", package_name)
	# Try exact match first
	if _packages.has(package_name):
		print("Found exact package match")
		return true
		
	# Try with parent packages
	for full_name in _packages:
		if full_name.ends_with("." + package_name):
			print("Found package as: ", full_name)
			return true
	
	print("Package not found")
	print("Currently loaded packages: ", _packages.keys())
	return false

func get_package_metadata(package_name: String) -> Dictionary:
	if not _initialized:
		initialize()
		
	print("\nGetting metadata for package: ", package_name)
	# Try exact match first
	var metadata = _packages.get(package_name, {})
	if not metadata.is_empty():
		print("Found metadata with exact match")
		print(metadata)
		return metadata
		
	# Try with parent packages
	for full_name in _packages:
		if full_name.ends_with("." + package_name):
			metadata = _packages[full_name]
			print("Found metadata as: ", full_name)
			print(metadata)
			return metadata
	
	print("No metadata found")
	print("Currently loaded packages: ", _packages.keys())
	return {}

func get_component(path: String) -> Dictionary:
	if not _initialized:
		initialize()
		
	print("\nGetting component: ", path)
	var component = _components.get(path, {})
	if not component.is_empty():
		print("Found component:")
		print(component)
	else:
		print("Component not found")
	return component

func get_loaded_packages() -> Array:
	if not _initialized:
		initialize()
		
	var packages = _packages.keys()
	print("\nLoaded packages: ", packages)
	return packages

func clear() -> void:
	print("\nClearing all packages and components")
	_packages.clear()
	_components.clear()
	_package_hierarchy.clear()
	_initialized = false
	print("Package manager cleared")
