class_name ModelManager
extends Node

signal models_loaded_changed
signal model_loaded(model_data: Dictionary)
signal loading_progress(progress: float, message: String)

var _parser: MOParser
var _models: Dictionary = {}  # Path -> Model data
var _model_tree: Dictionary = {}  # Package hierarchy
var _cache_file: String = "user://modelica_cache.json"
var components: Array[ModelicaComponent] = []
var equation_system: EquationSystem
var time: float = 0.0
var dt: float = 0.01  # Time step

func _init() -> void:
	_models = {}
	_model_tree = {}
	_parser = MOParser.new()
	equation_system = EquationSystem.new()

func _enter_tree() -> void:
	if not equation_system:
		equation_system = EquationSystem.new()
	if not equation_system.get_parent():
		add_child(equation_system)

func _ready() -> void:
	_clear_cache()
	
	# Get the absolute path to MSL directory
	var project_root = ProjectSettings.globalize_path("res://")
	var msl_path = project_root.path_join("apps/modelica_godot/MSL")
	
	# Start loading MSL asynchronously
	if DirAccess.dir_exists_absolute(msl_path):
		load_msl_directory.call_deferred(msl_path)
	else:
		push_error("MSL directory not found at: " + msl_path)
		# Try relative path as fallback
		msl_path = "res://apps/modelica_godot/MSL"
		if DirAccess.dir_exists_absolute(msl_path):
			load_msl_directory.call_deferred(msl_path)
		else:
			push_error("MSL directory not found at relative path either: " + msl_path)

func load_msl_directory(path: String) -> void:
	print("Loading MSL directory: ", path)
	
	# Focus only on Modelica subdirectory
	var modelica_path = path.path_join("Modelica")
	if not DirAccess.dir_exists_absolute(modelica_path):
		push_error("Modelica directory not found at: " + modelica_path)
		return
	
	# Collect files first
	emit_signal("loading_progress", 0.0, "Collecting files from Modelica directory...")
	var files_to_process = await _collect_files(modelica_path)
	if files_to_process.is_empty():
		print("No files to process in: ", modelica_path)
		return
		
	print("Total files to process: ", files_to_process.size())
	
	# Process files in smaller chunks
	const CHUNK_SIZE = 3
	var total_files = files_to_process.size()
	var processed = 0
	var skipped = 0
	var errors = []
	
	while processed + skipped < total_files:
		var chunk_end = mini(processed + skipped + CHUNK_SIZE, total_files)
		var chunk = files_to_process.slice(processed + skipped, chunk_end)
		
		for file_path in chunk:
			print("\nProcessing file ", processed + skipped + 1, " of ", total_files, ": ", file_path)
			
			var success = await _process_single_file(file_path, processed, total_files)
			if success:
				processed += 1
				print("Successfully processed file ", processed, " of ", total_files)
			else:
				skipped += 1
				errors.append(file_path)
				print("Skipped file ", file_path, " (Total skipped: ", skipped, ")")
			
			# Update progress after each file
			var progress = float(processed) / float(total_files - skipped)
			var message = "Loading Modelica models... (%d processed, %d skipped, %d total)" % [processed, skipped, total_files]
			emit_signal("loading_progress", progress, message)
			
			# Allow a frame to process
			await get_tree().process_frame
		
		# Allow a longer pause between chunks
		await get_tree().create_timer(0.1).timeout
	
	# Print summary
	print("\nModel loading complete:")
	print("- Total files: ", total_files)
	print("- Processed: ", processed)
	print("- Skipped: ", skipped)
	print("- Models in tree: ", _models.size())
	
	if not errors.is_empty():
		print("\nFiles with errors:")
		for error_file in errors:
			print("- ", error_file)
	
	_validate_loaded_models()
	emit_signal("models_loaded_changed")

func _clear_cache() -> void:
	if FileAccess.file_exists(_cache_file):
		DirAccess.remove_absolute(_cache_file)
		print("DEBUG: Cache cleared")

func _find_mo_files(path: String, results: Array) -> void:
	print("DEBUG: Searching for .mo files in: ", path)
	var dir = DirAccess.open(path)
	if dir:
		dir.include_hidden = false
		dir.include_navigational = false
		dir.list_dir_begin()
		
		while true:
			var file_name = dir.get_next()
			if file_name == "":
				break
				
			if file_name.begins_with("."):
				continue
				
			var full_path = path.path_join(file_name)
			print("DEBUG: Found: ", full_path)
			
			if dir.current_is_dir():
				_find_mo_files(full_path, results)
			elif file_name.ends_with(".mo"):
				print("DEBUG: Adding .mo file: ", full_path)
				results.append(full_path)
		
		dir.list_dir_end()
	else:
		push_error("Failed to open directory: " + path)

func _add_to_model_tree(path: String, model_data: Dictionary) -> void:
	# Extract the relative path from MSL directory
	var msl_index = path.find("/MSL/")
	if msl_index == -1:
		push_error("Invalid model path: " + path)
		return
		
	var relative_path = path.substr(msl_index + 5)  # Skip "/MSL/"
	var parts = relative_path.split("/")
	var current = _model_tree
	
	# Build the tree structure
	for i in range(parts.size() - 1):  # Skip the last part (filename)
		var part = parts[i]
		if not current.has(part):
			current[part] = {
				"name": part,
				"type": "package",
				"children": {}
			}
		current = current.get(part).get("children")
	
	# Add model data to the tree
	var filename = parts[-1].get_basename()
	if filename == "package":
		# For package.mo, merge with parent node
		current.merge({
			"description": model_data.get("description", ""),
			"name": model_data.get("name", ""),
			"path": path,
			"type": model_data.get("type", "")
		})
	else:
		# For other files, add as a child
		current[filename] = {
			"description": model_data.get("description", ""),
			"name": model_data.get("name", filename),
			"path": path,
			"type": model_data.get("type", "unknown"),
			"children": {}
		}

func _save_to_cache() -> void:
	# Don't save empty data
	if _models.is_empty() or _model_tree.is_empty():
		print("DEBUG: Not saving empty data to cache")
		return
		
	var cache = FileAccess.open(_cache_file, FileAccess.WRITE)
	if cache:
		var cache_data = {
			"models": _models,
			"tree": _model_tree,
			"timestamp": Time.get_unix_time_from_system()
		}
		var json_str = JSON.stringify(cache_data)
		cache.store_string(json_str)
		print("DEBUG: Saved to cache. Model count: ", _models.size())

func _load_from_cache() -> bool:
	if not FileAccess.file_exists(_cache_file):
		print("DEBUG: No cache file found")
		return false
		
	var cache = FileAccess.open(_cache_file, FileAccess.READ)
	if not cache:
		print("DEBUG: Failed to open cache file")
		return false
		
	var json = JSON.new()
	var error = json.parse(cache.get_as_text())
	if error != OK:
		print("DEBUG: Failed to parse cache JSON")
		return false
		
	var data = json.get_data()
	# Check if cache is less than a day old
	if Time.get_unix_time_from_system() - data.get("timestamp", 0) > 86400:
		print("DEBUG: Cache is too old")
		return false
		
	# Verify cache data
	if not data.has("models") or not data.has("tree"):
		print("DEBUG: Cache data is incomplete")
		return false
		
	# Check if the model tree is empty
	if data.get("tree", {}).is_empty():
		print("DEBUG: Cache contains empty model tree")
		return false
		
	_models = data.get("models", {})
	_model_tree = data.get("tree", {})
	
	print("DEBUG: Successfully loaded from cache. Model count: ", _models.size())
	print("DEBUG: Model tree from cache: ", _model_tree.keys())
	return true

func get_model_tree() -> Dictionary:
	return _model_tree

func get_model_data(path: String) -> Dictionary:
	return _models.get(path, {})

func search_models(query: String) -> Array:
	var results = []
	query = query.to_lower()
	for path in _models:
		var model = _models[path]
		if _model_matches_search(model, query):
			results.append({
				"path": path,
				"model": model
			})
	return results

func _model_matches_search(model: Dictionary, query: String) -> bool:
	# Check name
	if model.get("name", "").to_lower().contains(query):
		return true
	
	# Check description
	if model.get("description", "").to_lower().contains(query):
		return true
	
	return false

func add_component(component: ModelicaComponent) -> void:
	components.append(component)
	add_child(component)
	
	# Add component equations to system
	for eq in component.get_equations():
		equation_system.add_equation(eq, component)

func get_component(node_name: StringName) -> ModelicaComponent:
	# Convert StringName to NodePath
	var node_path = NodePath(node_name)
	# Find component by name
	for component in components:
		if component.name == node_name:
			return component
	return null

func connect_components(from_component: ModelicaComponent, from_port: String, 
						to_component: ModelicaComponent, to_port: String) -> bool:
	# Verify connection compatibility
	var from_connector = from_component.get_connector(from_port)
	var to_connector = to_component.get_connector(to_port)
	
	if from_connector.type != to_connector.type:
		push_error("Cannot connect different connector types")
		return false
	
	# Add connection equations
	for var_name in from_connector.variables.keys():
		if _is_across_variable(var_name):
			# Across variables are equal
			equation_system.add_equation(
				"%s.%s.%s = %s.%s.%s" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
		else:
			# Through variables sum to zero
			equation_system.add_equation(
				"%s.%s.%s + %s.%s.%s = 0" % [
					from_component.name, from_port, var_name,
					to_component.name, to_port, var_name
				],
				null
			)
	return true

func disconnect_components(from_component: ModelicaComponent, from_port: String,
						 to_component: ModelicaComponent, to_port: String) -> bool:
	# TODO: Remove connection equations
	return true

func _is_across_variable(var_name: String) -> bool:
	return var_name in ["position", "voltage", "temperature", "pressure"]

func simulate(duration: float) -> void:
	var steps = int(duration / dt)
	for i in range(steps):
		time += dt
		equation_system.solve() 

func _validate_loaded_models() -> void:
	print("Validating loaded models...")
	print("Model count: ", _models.size())
	print("Model tree size: ", _model_tree.size())
	
	# Validate model tree structure
	var has_modelica = _model_tree.has("Modelica")
	print("Has Modelica package: ", has_modelica)
	
	if has_modelica:
		var modelica = _model_tree["Modelica"]
		print("Modelica package type: ", modelica.get("type", "unknown"))
		print("Modelica subpackages: ", modelica.get("children", {}).keys())
	
	# Check for common packages
	var packages = ["Blocks", "Electrical", "Mechanics", "Thermal"]
	for pkg in packages:
		var has_pkg = _model_tree.has("Modelica") and _model_tree["Modelica"].get("children", {}).has(pkg)
		print("Has ", pkg, " package: ", has_pkg) 

func _collect_files(path: String) -> Array:
	var files_to_process = []
	var subdirs_to_process = []
	
	var dir = DirAccess.open(path)
	if not dir:
		push_error("Failed to open directory: " + path)
		return files_to_process
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	
	while file_name != "":
		var full_path = path.path_join(file_name)
		
		if dir.current_is_dir() and not file_name.begins_with("."):
			if not file_name in ["Resources", ".CI", ".git", "Images"]:  # Skip non-model directories
				subdirs_to_process.append(full_path)
		elif file_name.ends_with(".mo"):
			if not file_name in ["package.order"]:  # Skip non-model files
				files_to_process.append(full_path)
		
		file_name = dir.get_next()
	
	dir.list_dir_end()
	
	# Process subdirectories recursively
	for subdir in subdirs_to_process:
		var subdir_files = await _collect_files(subdir)
		files_to_process.append_array(subdir_files)
	
	return files_to_process

func _process_single_file(file_path: String, processed: int, total_files: int) -> bool:
	print("Starting to process file: ", file_path)
	
	# Skip if file doesn't exist
	if not FileAccess.file_exists(file_path):
		push_error("File not found: " + file_path)
		return false
	
	# Add file size check
	var file = FileAccess.open(file_path, FileAccess.READ)
	if not file:
		push_error("Failed to open file: " + file_path)
		return false
		
	var file_size = file.get_length()
	print("File size: ", file_size, " bytes")
	
	# Skip extremely large files
	if file_size > 1 * 1024 * 1024:  # 1MB limit
		push_error("File too large to process: " + file_path + " (" + str(file_size) + " bytes)")
		return false
	
	# Allow a frame to process
	await get_tree().process_frame
	
	print("Starting parse of file: ", file_path)
	var parse_start_time = Time.get_unix_time_from_system()
	
	var model_data = _parser.parse_file(file_path)
	
	var parse_duration = Time.get_unix_time_from_system() - parse_start_time
	print("Parse duration: ", parse_duration, " seconds")
	
	if parse_duration > 5.0:  # Log warning for slow parsing
		push_warning("Slow file parsing: " + file_path + " took " + str(parse_duration) + " seconds")
		return false  # Skip files that take too long to parse
	
	if model_data.is_empty():
		push_error("Failed to parse file: " + file_path)
		return false
		
	print("Adding to model tree: ", file_path)
	_models[file_path] = model_data
	_add_to_model_tree(file_path, model_data)
	
	# Emit signal with the model data
	emit_signal("model_loaded", model_data)
	
	print("Successfully processed file: ", file_path)
	return true 
