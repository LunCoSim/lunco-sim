extends MenuBar

signal new_graph_requested
signal save_requested
signal load_requested
signal save_to_file_requested(path: String)
signal load_from_file_requested(path: String)

var save_dialog: FileDialog
var load_dialog: FileDialog

func _ready() -> void:
	# Set menu titles
	self.set_menu_title(0, "File")
	self.set_menu_title(1, "NFT")
	
	# Connect the menu items
	%FileMenu.connect("id_pressed", _on_file_menu_pressed)
	
	# Setup file dialogs
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

func _on_file_menu_pressed(id: int) -> void:
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
