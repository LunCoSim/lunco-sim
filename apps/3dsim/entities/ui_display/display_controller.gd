extends Node

# Paths to the displays in the scene
@export var supply_chain_display_path: NodePath
@export var modelica_display_path: NodePath

# References to the display nodes
var supply_chain_display = null
var modelica_display = null

# Path to the avatar node (assuming it exists in the scene)
@export var avatar_path: NodePath

func _ready():
	# Initialize display references
	if supply_chain_display_path:
		supply_chain_display = get_node(supply_chain_display_path)
		if supply_chain_display:
			supply_chain_display.visible = false
			
	if modelica_display_path:
		modelica_display = get_node(modelica_display_path)
		if modelica_display:
			modelica_display.visible = false
	
	# Connect to the avatar's UiDisplayManager when the scene is ready
	call_deferred("_connect_to_avatar")

func _connect_to_avatar():
	if avatar_path:
		var avatar = get_node_or_null(avatar_path)
		if avatar:
			# Find the UiDisplayManager in the avatar
			var display_manager = avatar.get_node_or_null("UiDisplayManager")
			
			# If the avatar has a UiDisplayManager, connect our displays to it
			if display_manager and display_manager is LCUiDisplayManager:
				display_manager.set_displays(supply_chain_display, modelica_display)
				print("DisplayController: Successfully connected displays to Avatar's UiDisplayManager")
			else:
				print("DisplayController: Avatar does not have a UiDisplayManager component")
		else:
			print("DisplayController: Could not find Avatar at path: ", avatar_path)
	else:
		print("DisplayController: No avatar_path specified")

# Toggle the supply chain display directly (for backwards compatibility or custom control)
func toggle_supply_chain_display():
	if supply_chain_display:
		supply_chain_display.toggle_display()

# Toggle the modelica display directly (for backwards compatibility or custom control)
func toggle_modelica_display():
	if modelica_display:
		modelica_display.toggle_display() 