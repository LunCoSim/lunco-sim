@tool
class_name ModelBrowser
extends Control

signal model_selected(model_path: String, model_data: Dictionary)

@onready var tree: Tree = $VBoxContainer/HSplitContainer/Tree
@onready var search_box: LineEdit = $VBoxContainer/SearchBar
@onready var details_text: TextEdit = $VBoxContainer/HSplitContainer/Details
@onready var progress_bar: ProgressBar = $VBoxContainer/ProgressBar
@onready var status_label: Label = $VBoxContainer/StatusLabel

var model_manager: ModelManager

# Create colored icons
var model_icon: ImageTexture
var connector_icon: ImageTexture
var package_icon: ImageTexture
var unknown_icon: ImageTexture
var folder_icon: ImageTexture

func _ready() -> void:
	print("ModelBrowser: Ready")
	_create_icons()
	
	# Connect signals
	search_box.text_changed.connect(_on_search_text_changed)
	tree.item_selected.connect(_on_tree_item_selected)
	
	# Setup progress bar
	progress_bar.hide()
	progress_bar.min_value = 0
	progress_bar.max_value = 1.0
	
	# Setup details text
	details_text.editable = false

func initialize(manager: ModelManager) -> void:
	print("ModelBrowser: Initializing with manager")
	model_manager = manager
	
	# Connect to model manager signals
	model_manager.models_loaded.connect(_on_models_loaded)
	model_manager.model_loaded.connect(_on_model_loaded)
	model_manager.loading_progress.connect(_on_loading_progress)
	
	# Update UI
	status_label.text = "Ready"
	progress_bar.hide()
	
	# Get the absolute path to MSL directory
	var project_root = ProjectSettings.globalize_path("res://")
	var msl_path = project_root.path_join("apps/modelica_godot/MSL")
	print("ModelBrowser: MSL path: ", msl_path)
	
	# Start loading MSL
	if DirAccess.dir_exists_absolute(msl_path):
		print("ModelBrowser: MSL directory exists, starting load")
		model_manager.load_msl_directory(msl_path)
	else:
		push_error("MSL directory not found at: " + msl_path)
		# Try relative path as fallback
		msl_path = "res://apps/modelica_godot/MSL"
		if DirAccess.dir_exists_absolute(msl_path):
			print("ModelBrowser: Found MSL at relative path, starting load")
			model_manager.load_msl_directory(msl_path)
		else:
			push_error("MSL directory not found at relative path either: " + msl_path)

func _create_icons() -> void:
	# Create colored icons
	model_icon = _create_colored_icon(Color(0.2, 0.6, 1.0))  # Light blue
	connector_icon = _create_colored_icon(Color(0.8, 0.2, 0.2))  # Red
	package_icon = _create_colored_icon(Color(0.2, 0.8, 0.2))  # Green
	unknown_icon = _create_colored_icon(Color(0.7, 0.7, 0.7))  # Gray
	folder_icon = _create_colored_icon(Color(0.8, 0.8, 0.2))  # Yellow

func _create_colored_icon(color: Color) -> ImageTexture:
	var image := Image.create(16, 16, false, Image.FORMAT_RGBA8)
	image.fill(color)
	
	# Add a border
	for x in range(16):
		image.set_pixel(x, 0, Color.BLACK)
		image.set_pixel(x, 15, Color.BLACK)
		image.set_pixel(0, x, Color.BLACK)
		image.set_pixel(15, x, Color.BLACK)
	
	return ImageTexture.create_from_image(image)

func _on_models_loaded() -> void:
	print("ModelBrowser: Models loaded, updating tree")
	_update_tree()
	status_label.text = "Models loaded"
	progress_bar.hide()
	
	# Debug: Print model tree structure
	var model_tree = model_manager._model_tree
	print("ModelBrowser: Model tree structure:")
	print(JSON.stringify(model_tree, "  "))

func _on_model_loaded(model_data: Dictionary) -> void:
	print("ModelBrowser: Model loaded: ", model_data.get("name", "unnamed"))

func _on_loading_progress(progress: float, message: String) -> void:
	print("ModelBrowser: Loading progress: ", progress, " - ", message)
	progress_bar.show()
	progress_bar.value = progress
	status_label.text = message

func _on_search_text_changed(new_text: String) -> void:
	_update_tree(new_text)

func _on_tree_item_selected() -> void:
	var selected := tree.get_selected()
	if not selected:
		return
		
	var model_path = selected.get_metadata(0)
	if not model_path:
		return
		
	var model_data = model_manager.get_model_data(model_path)
	if model_data.is_empty():
		return
		
	# Update details
	details_text.text = _format_model_details(model_data)
	
	# Emit signal
	emit_signal("model_selected", model_path, model_data)

func _update_tree(filter: String = "") -> void:
	print("ModelBrowser: Updating tree with filter: ", filter)
	tree.clear()
	var root := tree.create_item()
	root.set_text(0, "Modelica")
	root.set_icon(0, package_icon)
	
	if not model_manager:
		print("ModelBrowser: No model manager available")
		return
		
	var model_tree = model_manager._model_tree
	print("ModelBrowser: Got model tree: ", model_tree.size(), " items")
	
	if model_tree.is_empty():
		print("ModelBrowser: Model tree is empty")
		return
		
	if model_tree.has("Modelica"):
		print("ModelBrowser: Found Modelica package, populating tree")
		_populate_tree(root, model_tree["Modelica"], filter.to_lower())
	else:
		print("ModelBrowser: No Modelica package found in tree")
		# Try to populate with the entire tree as fallback
		_populate_tree(root, model_tree, filter.to_lower())

func _populate_tree(parent: TreeItem, data: Dictionary, filter: String) -> void:
	print("ModelBrowser: Populating tree node: ", parent.get_text(0))
	print("ModelBrowser: Data to populate: ", data.keys())
	
	# Sort keys to ensure consistent ordering
	var keys = data.keys()
	keys.sort()
	
	for key in keys:
		if key in ["type", "name", "path", "description", "connectors", "parameters"]:
			continue  # Skip metadata keys
			
		var value = data[key]
		if not value is Dictionary:
			print("ModelBrowser: Skipping non-dictionary value for key: ", key)
			continue
			
		var should_show = filter.is_empty() or key.to_lower().contains(filter)
		
		# Create tree item
		var item := tree.create_item(parent)
		item.set_text(0, key)
		item.set_metadata(0, value.get("path", ""))
		
		var type = value.get("type", "")
		if type.is_empty() and value.has("path"):
			# Try to infer type from path
			var path = value.get("path", "")
			if path.ends_with("package.mo"):
				type = "package"
			else:
				type = "model"
		
		print("ModelBrowser: Added item: ", key, " of type: ", type)
		
		match type:
			"model", "block", "function", "record":
				item.set_icon(0, model_icon)
			"connector":
				item.set_icon(0, connector_icon)
			"package":
				item.set_icon(0, package_icon)
				# Recursively populate package contents
				_populate_tree(item, value, filter)
			_:
				if value.keys().size() > 0:
					# This is probably a folder
					item.set_icon(0, folder_icon)
					_populate_tree(item, value, filter)
				else:
					item.set_icon(0, unknown_icon)
		
		# Hide if filtered out
		item.visible = should_show or _has_visible_children(item)

func _has_visible_children(item: TreeItem) -> bool:
	var child = item.get_first_child()
	while child:
		if child.visible or _has_visible_children(child):
			return true
		child = child.get_next()
	return false

func _format_model_details(model: Dictionary) -> String:
	var details := ""
	
	# Basic info
	details += "Type: " + model.get("type", "unknown") + "\n"
	details += "Name: " + model.get("name", "unnamed") + "\n"
	
	if not model.get("description", "").is_empty():
		details += "\nDescription:\n" + model.description + "\n"
	
	# Parameters
	var params = model.get("parameters", [])
	if not params.is_empty():
		details += "\nParameters:\n"
		for param in params:
			details += "- " + param.get("name", "unnamed") + ": " + str(param.get("value", 0.0)) + "\n"
	
	# Variables
	var vars = model.get("variables", [])
	if not vars.is_empty():
		details += "\nVariables:\n"
		for var_def in vars:
			details += "- " + var_def.get("name", "unnamed") + ": " + var_def.get("type", "unknown") + "\n"
	
	return details 
