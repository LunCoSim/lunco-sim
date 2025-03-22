extends Node

# Paths to the displays in the scene
@export var supply_chain_display_path: NodePath
@export var modelica_display_path: NodePath

# References to the display nodes
var supply_chain_display = null
var modelica_display = null

# Track which display is currently active
var active_display = "none"  # "none", "supply_chain", or "modelica"

func _ready():
	if supply_chain_display_path:
		supply_chain_display = get_node(supply_chain_display_path)
		if supply_chain_display:
			supply_chain_display.visible = false
			
	if modelica_display_path:
		modelica_display = get_node(modelica_display_path)
		if modelica_display:
			modelica_display.visible = false

func _input(event):
	# Toggle supply chain display when pressing Tab key
	if event is InputEventKey and event.pressed and event.keycode == KEY_TAB:
		toggle_supply_chain_display()
		
	# Toggle modelica display when pressing M key
	if event is InputEventKey and event.pressed and event.keycode == KEY_M:
		toggle_modelica_display()

func toggle_supply_chain_display():
	if active_display == "supply_chain":
		# Hide display
		if supply_chain_display:
			supply_chain_display.toggle_display()
		active_display = "none"
	else:
		# First hide any active display
		if active_display == "modelica" and modelica_display:
			modelica_display.toggle_display()
		
		# Then show supply chain display
		if supply_chain_display:
			supply_chain_display.toggle_display()
		active_display = "supply_chain"

func toggle_modelica_display():
	if active_display == "modelica":
		# Hide display
		if modelica_display:
			modelica_display.toggle_display()
		active_display = "none"
	else:
		# First hide any active display
		if active_display == "supply_chain" and supply_chain_display:
			supply_chain_display.toggle_display()
		
		# Then show modelica display
		if modelica_display:
			modelica_display.toggle_display()
		active_display = "modelica" 