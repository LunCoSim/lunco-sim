extends Control

## Main controller for the Visual Command UI.

@onready var target_list = %TargetList
@onready var command_list = %CommandList
@onready var search_input = %SearchInput

var command_row_scene = preload("res://modules/command_ui/command_row.tscn")
var all_definitions: Dictionary = {}
var selected_target: String = ""

func _ready():
	search_input.text_changed.connect(_on_search_changed)
	target_list.item_selected.connect(_on_target_selected)
	%DetachButton.toggled.connect(_on_detach_toggled)
	
	# Connect to parent window to refresh data when shown
	var parent_win = get_parent()
	if parent_win is Window:
		parent_win.visibility_changed.connect(_on_window_visibility_changed)
	
	refresh_data()

func _on_window_visibility_changed():
	var parent_win = get_parent()
	if parent_win is Window and parent_win.visible:
		refresh_data()

func _on_detach_toggled(button_pressed: bool):
	# In Godot 4, you can't easily detach a single window if embedding is global,
	# but we can toggle the global setting for a "pop out" effect for all windows.
	# We must set this on the root viewport, not the window's viewport.
	get_tree().root.gui_embed_subwindows = !button_pressed
	print("[LCCommandUI] Detach toggled. Subwindows embedded: ", get_tree().root.gui_embed_subwindows)

func refresh_data():
	all_definitions = LCCommandRouter.get_all_command_definitions()
	_update_target_list()

func _update_target_list(filter: String = ""):
	target_list.clear()
	for target in all_definitions.keys():
		if filter == "" or filter.to_lower() in target.to_lower():
			var idx = target_list.add_item(target)
			target_list.set_item_metadata(idx, target)
	
	if target_list.item_count > 0:
		target_list.select(0)
		_on_target_selected(0)

func _on_target_selected(index: int):
	selected_target = target_list.get_item_metadata(index)
	_update_command_list()

func _update_command_list():
	# Clear existing
	for child in command_list.get_children():
		child.queue_free()
	
	var commands = all_definitions.get(selected_target, [])
	for cmd_info in commands:
		var row = command_row_scene.instantiate()
		command_list.add_child(row)
		row.setup(selected_target, cmd_info)

func _on_search_changed(new_text: String):
	_update_target_list(new_text)

