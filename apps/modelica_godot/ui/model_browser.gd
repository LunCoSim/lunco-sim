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

func _ready():
	print("ModelBrowser: Ready")
	# Create colored icons
	model_icon = _create_colored_icon(Color(0.2, 0.6, 1.0))  # Light blue
	connector_icon = _create_colored_icon(Color(0.8, 0.2, 0.2))  # Red
	package_icon = _create_colored_icon(Color(0.2, 0.8, 0.2))  # Green
	unknown_icon = _create_colored_icon(Color(0.7, 0.7, 0.7))  # Gray
	folder_icon = _create_colored_icon(Color(0.8, 0.8, 0.2))  # Yellow
	
	# Connect signals
	search_box.text_changed.connect(_on_search_text_changed)
	tree.item_selected.connect(_on_tree_item_selected)
	
	# Setup progress bar
	progress_bar.hide()
	progress_bar.min_value = 0
	progress_bar.max_value = 1.0
	
	# Setup details text
	details_text.editable = false

func _create_colored_icon(color: Color) -> ImageTexture:
	var image = Image.create(16, 16, false, Image.FORMAT_RGBA8)
	image.fill(color)
	
	# Add a border
	for x in range(16):
		image.set_pixel(x, 0, Color.BLACK)
		image.set_pixel(x, 15, Color.BLACK)
		image.set_pixel(0, x, Color.BLACK)
		image.set_pixel(15, x, Color.BLACK)
	
	return ImageTexture.create_from_image(image)

func initialize(manager: ModelManager):
	print("ModelBrowser: Initializing with manager")
	model_manager = manager
	model_manager.models_loaded.connect(_on_models_loaded)
	model_manager.loading_progress.connect(_on_loading_progress)
	
	# Get the absolute path to MSL directory
	var msl_path = ProjectSettings.globalize_path("res://apps/modelica_godot/MSL")
	print("ModelBrowser: Loading MSL from path: ", msl_path)
	
	# Start loading MSL
	progress_bar.show()
	model_manager.load_msl_directory(msl_path)

func _on_loading_progress(progress: float, message: String):
	print("ModelBrowser: Loading progress: ", progress, " - ", message)
	progress_bar.value = progress
	status_label.text = message

func _on_models_loaded():
	print("ModelBrowser: Models loaded")
	progress_bar.hide()
	status_label.text = "Models loaded"
	_populate_tree()

func _populate_tree():
	print("ModelBrowser: Populating tree")
	tree.clear()
	var root = tree.create_item()
	root.set_text(0, "Modelica Standard Library")
	root.set_icon(0, folder_icon)
	
	var model_tree = model_manager.get_model_tree()
	print("ModelBrowser: Model tree data: ", model_tree)
	
	# Start with Modelica package if it exists
	if model_tree.has("Modelica"):
		var modelica_data = model_tree["Modelica"]
		if modelica_data is Dictionary:
			if modelica_data.has("path"):
				# This is the root package itself
				root.set_metadata(0, modelica_data)
				root.set_icon(0, package_icon)
				# Add its children
				for key in modelica_data:
					if key not in ["path", "type", "name", "description"]:
						_add_tree_items(root, {key: modelica_data[key]})
			else:
				# This is just a container
				_add_tree_items(root, modelica_data)

func _add_tree_items(parent: TreeItem, data: Dictionary):
	for key in data:
		var item = tree.create_item(parent)
		var node_data = data[key]
		
		if node_data is Dictionary:
			if node_data.has("path"):
				# This is a model leaf
				item.set_text(0, node_data.get("name", key))
				item.set_metadata(0, node_data)
				print("ModelBrowser: Adding model leaf: ", key, " type: ", node_data.get("type", "unknown"))
				
				# Set icon based on type
				match node_data.get("type", ""):
					"model":
						item.set_icon(0, model_icon)
					"connector":
						item.set_icon(0, connector_icon)
					"package":
						item.set_icon(0, package_icon)
						# For packages, also add their children
						for child_key in node_data:
							if child_key not in ["path", "type", "name", "description"]:
								_add_tree_items(item, {child_key: node_data[child_key]})
					_:
						item.set_icon(0, unknown_icon)
			else:
				# This is a package/directory
				print("ModelBrowser: Adding package: ", key)
				item.set_text(0, key)
				item.set_icon(0, folder_icon)
				_add_tree_items(item, node_data)

func _on_tree_item_selected():
	var selected = tree.get_selected()
	if selected and selected.get_metadata(0):
		var metadata = selected.get_metadata(0)
		_show_model_details(metadata)
		emit_signal("model_selected", metadata.path, model_manager.get_model_data(metadata.path))

func _show_model_details(metadata: Dictionary):
	var details = """Type: {type}
Name: {name}
Path: {path}

Description:
{description}""".format(metadata)
	
	details_text.text = details

func _on_search_text_changed(new_text: String):
	if new_text.length() < 3:  # Only search with 3+ characters
		_populate_tree()
		return
	
	var results = model_manager.search_models(new_text)
	tree.clear()
	var root = tree.create_item()
	root.set_text(0, "Search Results")
	root.set_icon(0, folder_icon)
	
	for result in results:
		var item = tree.create_item(root)
		item.set_text(0, result.model.get("name", "Unknown"))
		item.set_metadata(0, {
			"path": result.path,
			"type": result.model.get("type", "unknown"),
			"name": result.model.get("name", "Unknown"),
			"description": result.model.get("description", "")
		}) 
