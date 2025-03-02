extends RefCounted

# MODELICAPATH - empty by default
var modelica_paths = []

# MODELICAPATH management functions
func add_modelica_path(path: String) -> void:
	if not path in modelica_paths:
		modelica_paths.append(path)
		print("Added to MODELICAPATH: " + path)

func remove_modelica_path(path: String) -> void:
	if path in modelica_paths:
		modelica_paths.erase(path)
		print("Removed from MODELICAPATH: " + path)

func clear_modelica_path() -> void:
	modelica_paths.clear()
	print("MODELICAPATH cleared")

func get_modelica_path() -> Array:
	return modelica_paths.duplicate()

# Basic model loading functions

# Load a model file by direct path
func load_model_file(path: String) -> String:
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		print("Error opening file: " + path)
		return ""
	
	var content = file.get_as_text()
	file.close()
	return content

# Extract package name from package.mo content
func extract_package_name(content: String) -> String:
	var regex = RegEx.new()
	regex.compile("package\\s+([A-Za-z0-9_]+)\\s")
	
	var match = regex.search(content)
	if match and match.get_group_count() >= 1:
		return match.get_string(1)
	
	return ""

# Discover package structure from a file path
func discover_package_from_path(file_path: String) -> Dictionary:
	var package_info = {
		"root_path": "",
		"root_package_name": "",
		"parent_path": "",
		"hierarchy": []
	}
	
	# Start from the directory containing the file
	var dir_path = file_path.get_base_dir()
	var package_hierarchy = []
	
	# Traverse up the directory structure looking for package.mo files
	while not dir_path.is_empty():
		var package_mo_path = dir_path.path_join("package.mo")
		
		if FileAccess.file_exists(package_mo_path):
			# Found a package.mo, extract its name
			var package_content = load_model_file(package_mo_path)
			var package_name = extract_package_name(package_content)
			
			if not package_name.is_empty():
				# Add to our hierarchy (at the beginning)
				package_hierarchy.insert(0, package_name)
				
				# If this is the first one we found (deepest in tree),
				# remember its path for validation
				if package_info["root_path"].is_empty():
					package_info["root_path"] = dir_path
					package_info["root_package_name"] = package_name
					package_info["parent_path"] = dir_path.get_base_dir()
		
		# Try to find parent package
		var parent_dir = dir_path.get_base_dir()
		if parent_dir == dir_path:
			# We've reached the root
			break
		
		dir_path = parent_dir
	
	# Set the full hierarchy
	package_info["hierarchy"] = package_hierarchy
	
	return package_info

# Auto-add the parent directory of a model's package to MODELICAPATH
func auto_add_package_path(model_path: String) -> Dictionary:
	var package_info = discover_package_from_path(model_path)
	var result = {"path_added": "", "success": false}
	
	if not package_info["parent_path"].is_empty():
		add_modelica_path(package_info["parent_path"])
		result["path_added"] = package_info["parent_path"]
		result["success"] = true
		print("Auto-detected and added to MODELICAPATH: " + package_info["parent_path"])
	
	return result

# Find a model by name using MODELICAPATH
func find_model_by_name(name: String) -> String:
	# If it's a direct path, check if it exists
	if FileAccess.file_exists(name):
		return name
	
	# If name ends with .mo, strip it for searching
	var model_name = name
	if model_name.ends_with(".mo"):
		model_name = model_name.substr(0, model_name.length() - 3)
	
	# Try in each MODELICAPATH directory
	for path in modelica_paths:
		# Try in root directory
		var file_path = path.path_join(model_name + ".mo")
		if FileAccess.file_exists(file_path):
			return file_path
		
		# Try in subdirectories (one level)
		var dir = DirAccess.open(path)
		if dir:
			dir.list_dir_begin()
			var dir_name = dir.get_next()
			
			while dir_name != "":
				if dir.current_is_dir() and not dir_name.begins_with("."):
					var subdir_path = path.path_join(dir_name)
					var model_path = subdir_path.path_join(model_name + ".mo")
					
					if FileAccess.file_exists(model_path):
						return model_path
				
				dir_name = dir.get_next()
			dir.list_dir_end()
	
	# Not found
	return ""

# Load a model by name
func load_model_by_name(name: String) -> String:
	var file_path = find_model_by_name(name)
	if file_path.is_empty():
		print("Model not found: " + name)
		return ""
	
	return load_model_file(file_path)

# Find a model by its fully qualified name
func find_model_by_qualified_name(qualified_name: String) -> String:
	var parts = qualified_name.split(".")
	
	# Search in MODELICAPATH
	for base_path in modelica_paths:
		var current_path = base_path
		var found = true
		
		# Navigate through package hierarchy
		for i in range(parts.size() - 1):
			var package_name = parts[i]
			
			# Try as directory with package.mo
			var dir_path = current_path.path_join(package_name)
			var package_mo_path = dir_path.path_join("package.mo")
			
			if FileAccess.file_exists(package_mo_path):
				current_path = dir_path
				continue
			
			# Try as single file
			var file_path = current_path.path_join(package_name + ".mo")
			if FileAccess.file_exists(file_path):
				# Can't navigate into a file-based package
				found = false
				break
			
			# Package not found
			found = false
			break
		
		if found:
			# Try to find the model file
			var model_name = parts[parts.size() - 1]
			var model_path = current_path.path_join(model_name + ".mo")
			
			if FileAccess.file_exists(model_path):
				return model_path
	
	# Not found
	return ""

# Load a model by its fully qualified name
func load_model_by_qualified_name(qualified_name: String) -> String:
	var file_path = find_model_by_qualified_name(qualified_name)
	if file_path.is_empty():
		print("Model not found by qualified name: " + qualified_name)
		return ""
	
	return load_model_file(file_path)

# Extract package information from a Modelica file
func extract_package_info(content: String) -> Dictionary:
	var package_info = {"within": ""}
	var regex = RegEx.new()
	regex.compile("within\\s+([A-Za-z0-9_\\.]+)\\s*;")
	
	var match = regex.search(content)
	if match and match.get_group_count() >= 1:
		package_info["within"] = match.get_string(1)
	
	return package_info

# Dependency Management

# Parse dependencies from a Modelica file content
func parse_dependencies(content: String) -> Array:
	var dependencies = []
	var regex = RegEx.new()
	regex.compile("uses\\s*\\(\\s*([A-Za-z0-9_\\.]+)\\s*\\(\\s*version\\s*=\\s*\"([^\"]+)\"\\s*\\)\\s*\\)")
	
	var matches = regex.search_all(content)
	for match in matches:
		if match.get_group_count() >= 2:
			dependencies.append({
				"name": match.get_string(1),
				"version": match.get_string(2)
			})
	
	return dependencies

# Load all dependencies for a model
func load_dependencies(model_content: String) -> Dictionary:
	var loaded = {}
	var deps = parse_dependencies(model_content)
	
	for dep in deps:
		var dep_name = dep["name"]
		if not loaded.has(dep_name):
			var dep_content = load_model_by_name(dep_name)
			if not dep_content.is_empty():
				loaded[dep_name] = {
					"content": dep_content,
					"version": dep["version"]
				}
				
				# Recursively load dependencies of this dependency
				var sub_deps = load_dependencies(dep_content)
				for sub_name in sub_deps:
					loaded[sub_name] = sub_deps[sub_name]
			else:
				print("Warning: Dependency not found: " + dep_name + " (version " + dep["version"] + ")")
	
	return loaded

# Error types for validation
enum ErrorType {
	DEPENDENCY_NOT_FOUND,
	VERSION_CONFLICT,
	INVALID_PACKAGE_STRUCTURE,
	MODEL_NOT_FOUND
}

# Error class for detailed error reporting
class PackageError:
	var type: int
	var message: String
	var details: Dictionary
	
	func _init(err_type: int, err_message: String, err_details: Dictionary = {}):
		type = err_type
		message = err_message
		details = err_details

# Validate and load a model with complete dependency checking
func validate_and_load_model(model_name: String) -> Dictionary:
	var result = {
		"success": false,
		"content": "",
		"errors": [],
		"dependencies": {},
		"package_info": {}
	}
	
	# Try to load the model
	var content = ""
	var model_path = ""
	
	# Check if it's a direct file path
	if FileAccess.file_exists(model_name):
		model_path = model_name
		content = load_model_file(model_name)
		
		# Auto-add the package path to MODELICAPATH
		auto_add_package_path(model_path)
	else:
		# Try as a simple name
		model_path = find_model_by_name(model_name)
		if not model_path.is_empty():
			content = load_model_file(model_path)
		
		# If not found, try as a qualified name
		if content.is_empty() and "." in model_name:
			model_path = find_model_by_qualified_name(model_name)
			if not model_path.is_empty():
				content = load_model_file(model_path)
	
	if content.is_empty():
		result["errors"].append(PackageError.new(
			ErrorType.MODEL_NOT_FOUND,
			"Model not found: " + model_name
		))
		return result
	
	result["content"] = content
	
	# Extract package info
	result["package_info"] = extract_package_info(content)
	
	# Validate package structure
	if not validate_package_structure(model_path, result["package_info"]):
		result["errors"].append(PackageError.new(
			ErrorType.INVALID_PACKAGE_STRUCTURE,
			"Invalid package structure for model: " + model_path,
			{"within": result["package_info"]["within"]}
		))
	
	# Load dependencies
	var deps = parse_dependencies(content)
	for dep in deps:
		var dep_result = validate_and_load_model(dep["name"])
		result["dependencies"][dep["name"]] = dep_result
		
		if not dep_result["success"]:
			for err in dep_result["errors"]:
				if err.type == ErrorType.MODEL_NOT_FOUND:
					result["errors"].append(PackageError.new(
						ErrorType.DEPENDENCY_NOT_FOUND,
						"Dependency not found: " + dep["name"],
						{"version": dep["version"]}
					))
	
	# Set success if no errors
	result["success"] = result["errors"].size() == 0
	
	return result

# Validate package structure based on "within" clause and file location
func validate_package_structure(model_path: String, package_info: Dictionary) -> bool:
	# If there's no "within" clause, it's valid (top-level model)
	if package_info["within"].is_empty():
		return true
	
	# Check the discovered package hierarchy against the within clause
	var discovered_info = discover_package_from_path(model_path)
	
	# If we couldn't discover any package, but there's a within clause,
	# it could still be valid if the within package exists 
	if discovered_info["hierarchy"].size() == 0:
		# Just check if the within package exists
		var within_path = find_model_by_qualified_name(package_info["within"])
		return not within_path.is_empty()
	
	# Compare the within clause with the discovered hierarchy
	var expected_within = ".".join(discovered_info["hierarchy"])
	
	# Allow partial match - model might be in a subpackage that's not fully represented in the hierarchy
	return package_info["within"] == expected_within or expected_within.begins_with(package_info["within"])

# Create a static function to get a new package manager
static func create() -> RefCounted:
	return load("res://apps/modelica/core/package_manager.gd").new() 