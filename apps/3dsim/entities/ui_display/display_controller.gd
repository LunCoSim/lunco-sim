extends Node

# Paths to the displays in the scene
@export var supply_chain_display_path: NodePath
@export var modelica_display_path: NodePath

# References to the display nodes
var supply_chain_display = null
var modelica_display = null

# Reference to the UI display manager
var ui_display_manager = null

# Path to the avatar node (assuming it exists in the scene)
@export var avatar_path: NodePath

func _ready():
	# Add to group for easy identification
	add_to_group("display_controller")
	
	# Initialize display references - DEFERRED to ensure nodes are ready
	call_deferred("_initialize_display_references")
	
	# Connect to the avatar's UiDisplayManager when the scene is ready
	call_deferred("_connect_to_avatar")

# Initialize display references after a frame to ensure they're loaded
func _initialize_display_references():
	print("DisplayController: Initializing display references")
	
	if supply_chain_display_path:
		supply_chain_display = get_node_or_null(supply_chain_display_path)
		if supply_chain_display:
			print("DisplayController: Found supply chain display at path: ", supply_chain_display_path)
			supply_chain_display.visible = false
		else:
			print("DisplayController: Could not find supply chain display at path: ", supply_chain_display_path)
			
	if modelica_display_path:
		modelica_display = get_node_or_null(modelica_display_path)
		if modelica_display:
			print("DisplayController: Found modelica display at path: ", modelica_display_path)
			modelica_display.visible = false
		else:
			print("DisplayController: Could not find modelica display at path: ", modelica_display_path)
	
	# Fallback: Try to find displays by group if paths didn't work
	if !modelica_display:
		var modelica_displays = get_tree().get_nodes_in_group("modelica_display")
		if modelica_displays.size() > 0:
			modelica_display = modelica_displays[0]
			print("DisplayController: Found modelica display by group: ", modelica_display)
	
	if !supply_chain_display:
		var supply_chain_displays = get_tree().get_nodes_in_group("supply_chain_display")
		if supply_chain_displays.size() > 0:
			supply_chain_display = supply_chain_displays[0]
			print("DisplayController: Found supply chain display by group: ", supply_chain_display)
			
	# After initializing references, attempt to connect to avatar if it exists
	_connect_to_avatar()

func _connect_to_avatar():
	var avatar = null
	
	# First try using the specified path
	if avatar_path:
		avatar = get_node_or_null(avatar_path)
		if avatar:
			print("DisplayController: Found Avatar at specified path: ", avatar_path)
	
	# If no path specified or path invalid, try finding the avatar in the scene
	if not avatar:
		print("DisplayController: No valid avatar_path, searching for avatar in scene...")
		
		# Try to find avatar by group
		var avatars = get_tree().get_nodes_in_group("avatar")
		if avatars.size() > 0:
			avatar = avatars[0]
			print("DisplayController: Found Avatar in 'avatar' group: ", avatar)
		
		# If still not found, try finding by class name
		if not avatar:
			var potential_avatars = get_tree().get_nodes_in_group("LCAvatar")
			if potential_avatars.size() > 0:
				avatar = potential_avatars[0]
				print("DisplayController: Found Avatar of class LCAvatar: ", avatar)
		
		# Final attempt - look for a node named "Avatar" in the scene
		if not avatar:
			avatar = get_tree().get_first_node_in_group("Avatar")
			if avatar:
				print("DisplayController: Found node named Avatar: ", avatar)
	
	if avatar:
		# Find the UiDisplayManager in the avatar
		ui_display_manager = avatar.get_node_or_null("UiDisplayManager")
		
		# If the avatar has a UiDisplayManager, connect our displays to it
		if ui_display_manager and ui_display_manager is LCUiDisplayManager:
			ui_display_manager.set_displays(supply_chain_display, modelica_display)
			print("DisplayController: Successfully connected displays to Avatar's UiDisplayManager: ", ui_display_manager)
		else:
			print("DisplayController: Avatar does not have a UiDisplayManager component: ", avatar)
	else:
		print("DisplayController: Could not find any Avatar node in the scene")

# Toggle the supply chain display directly (for backwards compatibility or custom control)
func toggle_supply_chain_display():
	if supply_chain_display:
		supply_chain_display.toggle_display()

# Toggle the modelica display directly (for backwards compatibility or custom control)
func toggle_modelica_display():
	if modelica_display:
		modelica_display.toggle_display() 