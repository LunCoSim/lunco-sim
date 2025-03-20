extends Node

# Path to the supply chain display in the scene
@export var supply_chain_display_path: NodePath

# Reference to the supply chain display node
var supply_chain_display = null

func _ready():
	if supply_chain_display_path:
		supply_chain_display = get_node(supply_chain_display_path)

func _input(event):
	# Toggle supply chain display when pressing Tab key
	if event is InputEventKey and event.pressed and event.keycode == KEY_TAB:
		if supply_chain_display:
			supply_chain_display.toggle_display() 