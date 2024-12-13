extends MenuButton

signal new_graph_requested
signal save_requested
signal load_requested
signal save_to_file_requested(path: String)
signal load_from_file_requested(path: String)

var popup: PopupMenu
var save_dialog: FileDialog
var load_dialog: FileDialog

func _ready():
	popup = get_popup()
	
	# Add menu items
	popup.add_item("New", 0)
	popup.add_separator()
	popup.add_item("Save", 1)
	popup.add_item("Load", 2)
	popup.add_separator()
	popup.add_item("Save to File", 3)
	popup.add_item("Load from File", 4)
	
	# Connect the signal
	popup.connect("id_pressed", _on_item_pressed)
	
	# Set initial text
	text = "File"
	
	# Create file dialogs
	_setup_file_dialogs()

func _setup_file_dialogs():
	# Save dialog
	save_dialog = FileDialog.new()
	save_dialog.access = FileDialog.ACCESS_FILESYSTEM
	save_dialog.file_mode = FileDialog.FILE_MODE_SAVE_FILE
	save_dialog.add_filter("*.json", "JSON Files")
	save_dialog.connect("file_selected", _on_save_file_selected)
	add_child(save_dialog)
	
	# Load dialog
	load_dialog = FileDialog.new()
	load_dialog.access = FileDialog.ACCESS_FILESYSTEM
	load_dialog.file_mode = FileDialog.FILE_MODE_OPEN_FILE
	load_dialog.add_filter("*.json", "JSON Files")
	load_dialog.connect("file_selected", _on_load_file_selected)
	add_child(load_dialog)

func _on_item_pressed(id: int):
	match id:
		0: # New
			emit_signal("new_graph_requested")
		1: # Save
			emit_signal("save_requested")
		2: # Load
			emit_signal("load_requested")
		3: # Save to File
			save_dialog.popup_centered(Vector2(800, 600))
		4: # Load from File
			load_dialog.popup_centered(Vector2(800, 600))

func _on_save_file_selected(path: String):
	emit_signal("save_to_file_requested", path)

func _on_load_file_selected(path: String):
	emit_signal("load_from_file_requested", path)
