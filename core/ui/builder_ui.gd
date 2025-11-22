extends Control

@onready var part_list = $PanelContainer/VBoxContainer/ScrollContainer/PartList

func _ready():
	var bm = get_node_or_null("/root/BuilderManager")
	if not bm:
		push_error("BuilderManager not found")
		return
		
	# Populate part list
	for part_id in bm.part_registry:
		var btn = Button.new()
		btn.text = part_id.capitalize()
		btn.pressed.connect(_on_part_selected.bind(part_id))
		part_list.add_child(btn)

func _on_part_selected(part_id: String):
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.select_part(part_id)

func _on_launch_button_pressed():
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.stop_building()
	# Logic to enable physics for all constructibles?
	# For now, just hide UI or switch mode
	visible = false
