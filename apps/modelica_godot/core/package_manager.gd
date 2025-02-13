@tool
extends Node
class_name PackageManager

signal package_loaded(package_name: String)
signal loading_error(package_name: String, error: String)

var _packages: Dictionary = {}
var _loading_queue: Array = []
var _is_loading: bool = false
var _package_tree: Dictionary = {}
var _package_cache: Dictionary = {}
var _imports: Dictionary = {}  # package_path -> Array[ImportInfo]
var _import_aliases: Dictionary = {}  # alias -> fully_qualified_name

class ImportInfo:
	var source_package: String  # Package containing the import
	var imported_name: String   # Full name being imported
	var alias: String          # Optional alias
	var is_wildcard: bool      # Whether it's a wildcard import
	
	func _init(source: String, name: String, alias_name: String = "", wildcard: bool = false):
		source_package = source
		imported_name = name
		alias = alias_name
		is_wildcard = wildcard

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

func has_package(path: String) -> bool:
	var parts = path.split(".")
	var current = _package_tree
	
	for part in parts:
		if not current.has(part):
			return false
		current = current.get(part)
	return true

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

func add_package(path: String, package_data: Dictionary) -> void:
	var parts = path.split("/")
	var current = _package_tree
	
	for part in parts:
		if not current.has(part):
			current[part] = {"children": {}, "models": {}}
		current = current.get(part).get("children")
	
	# Store package data
	current["package_info"] = package_data

func add_import(source_package: String, import_name: String, alias: String = "", is_wildcard: bool = false) -> void:
	if not _imports.has(source_package):
		_imports[source_package] = []
	
	var import_info = ImportInfo.new(source_package, import_name, alias, is_wildcard)
	_imports[source_package].append(import_info)
	
	if not alias.is_empty():
		_import_aliases[alias] = import_name

func resolve_type(type_name: String, current_package: String) -> Dictionary:
	# First try cache
	var cache_key = current_package + "." + type_name
	if _package_cache.has(cache_key):
		return _package_cache[cache_key]
	
	var resolved = {}
	
	# 1. Try as fully qualified name first
	if "." in type_name:
		resolved = _resolve_qualified_name(type_name)
		if not resolved.is_empty():
			_package_cache[cache_key] = resolved
			return resolved
	
	# 2. Try resolving in current package
	resolved = _resolve_in_package(type_name, current_package)
	if not resolved.is_empty():
		_package_cache[cache_key] = resolved
		return resolved
	
	# 3. Try resolving through imports
	resolved = _resolve_through_imports(type_name, current_package)
	if not resolved.is_empty():
		_package_cache[cache_key] = resolved
		return resolved
	
	# 4. Try resolving in parent packages
	var parent_package = current_package
	while "." in parent_package:
		parent_package = parent_package.substr(0, parent_package.rfind("."))
		resolved = _resolve_in_package(type_name, parent_package)
		if not resolved.is_empty():
			_package_cache[cache_key] = resolved
			return resolved
	
	# 5. Try in Modelica Standard Library as last resort
	resolved = _resolve_in_package(type_name, "Modelica")
	if not resolved.is_empty():
		_package_cache[cache_key] = resolved
		return resolved
	
	return {}

func _resolve_through_imports(type_name: String, current_package: String) -> Dictionary:
	# Check if we have any imports for this package
	if not _imports.has(current_package):
		return {}
	
	for import_info in _imports[current_package]:
		var resolved = {}
		
		if import_info.is_wildcard:
			# For wildcard imports, try resolving in the imported package
			resolved = _resolve_in_package(type_name, import_info.imported_name)
		else:
			# For specific imports, check if the name matches
			if import_info.alias.is_empty():
				# No alias - check if the imported name matches
				if import_info.imported_name.ends_with("." + type_name):
					resolved = _resolve_qualified_name(import_info.imported_name)
			else:
				# With alias - check if the alias matches
				if import_info.alias == type_name:
					resolved = _resolve_qualified_name(import_info.imported_name)
		
		if not resolved.is_empty():
			return resolved
	
	return {}

func _resolve_in_package(type_name: String, package_path: String) -> Dictionary:
	var current = _package_tree
	var parts = package_path.split(".")
	
	# Navigate to package
	for part in parts:
		if not current.has(part):
			return {}
		current = current.get(part)
	
	# Look for type in package
	if current.has("models") and current.get("models").has(type_name):
		return current.get("models").get(type_name)
	
	return {}

func _resolve_qualified_name(qualified_name: String) -> Dictionary:
	var parts = qualified_name.split(".")
	var type_name = parts[-1]
	var package_path = ".".join(parts.slice(0, -1))
	return _resolve_in_package(type_name, package_path)

func clear_cache() -> void:
	_package_cache.clear()
