extends Control

@onready var part_list = $PanelContainer/VBoxContainer/ScrollContainer/PartList
@onready var label = $PanelContainer/VBoxContainer/Label

var selected_button: Button = null

func _ready():
	var bm = get_node_or_null("/root/BuilderManager")
	if not bm:
		push_error("BuilderManager not found")
		return
		
	# Update label with instructions
	label.text = "Mission Builder\n[Select part, then click to place]"
	
	# Populate part list
	for part_id in bm.part_registry:
		var btn = Button.new()
		btn.text = part_id.capitalize().replace("_", " ")
		btn.pressed.connect(_on_part_selected.bind(part_id, btn))
		part_list.add_child(btn)

func _on_part_selected(part_id: String, btn: Button):
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.select_part(part_id)
		
		# Visual feedback - highlight selected button
		if selected_button:
			selected_button.modulate = Color(1, 1, 1)
		selected_button = btn
		btn.modulate = Color(0.5, 1.0, 0.5)
		
		# Update label
		if part_id == "chassis_box":
			label.text = "Chassis Box Selected\n[Click anywhere to place new rover]"
		else:
			label.text = part_id.capitalize().replace("_", " ") + " Selected\n[Click on existing rover to attach]"

func _on_launch_button_pressed():
	var bm = get_node_or_null("/root/BuilderManager")
	if bm:
		bm.stop_building()
	# Logic to enable physics for all constructibles?
	# For now, just hide UI or switch mode
	visible = false
