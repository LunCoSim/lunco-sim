## Avatar - The Player's Agent in the World
##
## Avatar serves as the intermediary between the USER and the SYSTEM, acting as the player's
## viewport and control interface into the simulation world.
##
## ARCHITECTURAL ROLE:
## - User Interface Layer: Handles how the player sees and interacts with the world
## - Input Coordinator: Manages when and how different input systems are active
## - Visualization Manager: Controls camera settings and view preferences
## - Entity Controller: Coordinates control requests and entity targeting
##
## RESPONSIBILITIES:
## 1. Camera Management
##    - Handles camera input (rotation, zoom)
##    - Manages camera settings per controller type
##    - Controls mouse capture mode for camera rotation
##
## 2. Input Coordination
##    - Enables/disables input adapters based on context
##    - Routes keyboard shortcuts (entity selection, spawning)
##    - Manages input priority (UI vs gameplay)
##
## 3. Entity Control
##    - Targets entities for control
##    - Sends control requests to ControlManager
##    - Manages control release
##
## 4. UI Management
##    - Owns and coordinates UI components
##    - Updates entity lists
##    - Manages UI display states
##
## 5. Raycasting & Interaction
##    - Handles click-to-select entities
##    - Manages NFT spawning interactions
##    - Coordinates with display managers
##
## DESIGN PHILOSOPHY:
## Avatar is CLIENT-SIDE ONLY and never synchronized over network.
## Each player has their own Avatar instance with their own camera and UI.
## Avatar coordinates but delegates implementation to specialized classes:
## - Input adapters handle controller-specific input
## - Controllers handle entity behavior
## - UI classes handle interface rendering
##
@icon("res://core/avatar/avatar.svg")
class_name LCAvatar
extends LCSpaceSystem

#===============================================================================
# SIGNALS - Communication with other systems
#===============================================================================
signal spawn_entity(entity, position)

signal target_changed(target)

signal requesting_control(entity_idx)
signal release_control

#===============================================================================
# CONSTANTS - Configuration values
#===============================================================================
const MOUSE_SENSITIVITY = 0.015
const RAY_LENGTH = 10000
const SPEED = 5.0
const JUMP_VELOCITY = 4.5
const ZOOM_SPEED = 0.1
const WHEEL_ZOOM_INCREMENT = 0.1  # Add default value for wheel zoom
const DEVICE_ID_REMOTE = 7 # Dedicated device ID for remote artificial events

#===============================================================================
# EXPORTS - Editor-configurable properties
#===============================================================================
@export var target: Node3D
@export var entity_to_spawn = EntitiesDB.Entities.Astronaut

@export var CATCH_CAMERA := true

#===============================================================================
# COMPONENTS - Child nodes and references
#===============================================================================
@onready var ui := $UI
@onready var camera:SpringArmCamera = $SpringArmCamera
@onready var ui_display_manager := $UiDisplayManager

#===============================================================================
# STATE - Internal runtime state
#===============================================================================
var mouse_control := false
var controller: LCController

#===============================================================================
# ENTITY TARGETING & CONTROL - Managing what the player controls
#===============================================================================

## Sets the target entity/controller for the player to control
## Handles controller resolution and camera exclusion
func set_target(_target):
	print("Set target: ", _target)
	if camera and target:
		camera.remove_excluded_object(controller.get_parent())
	
	if controller:
		var current_entity = controller.get_parent()
		var new_entity = _target
		
		# Resolve new_entity if _target is a controller
		if _target is LCController:
			new_entity = _target.get_parent()
			
		# Only release if we are switching to a DIFFERENT entity
		if current_entity != new_entity:
			release_control.emit(controller.get_parent().get_path())
	
	target = _target
	#searching for controller
	if not _target is LCController: 
		#TBD: Better way to find controller
		target = LCController.find_controller(_target)
		controller = LCController.find_controller(_target)
	
	if camera and controller:
		camera.add_excluded_object(controller.get_parent())
	
	# Calling state transited function
	_on_state_transited()
	return controller

## Sets the camera and makes it current if CATCH_CAMERA is enabled
func set_camera(_camera):
	camera = _camera
	if camera and CATCH_CAMERA:
		camera.set_current()

#===============================================================================
# INITIALIZATION - Setup and signal connections
#===============================================================================

## Called when Avatar enters the scene tree
## Sets up camera, target, and connects to ControlManager signals
func _ready():
	# Add to group for easier identification
	add_to_group("avatar")
	
	set_camera(camera)
	set_target(target)
	ControlManager.control_granted.connect(_on_control_granted)
	ControlManager.control_request_denied.connect(_on_control_request_denied)
	ControlManager.control_released.connect(ui._on_control_released)
	ControlManager.control_granted.connect(ui._on_control_granted)
	ControlManager.control_request_denied.connect(ui._on_control_request_denied)
	
	# Initialize the UiDisplayManager if it exists
	if not ui_display_manager and has_node("UiDisplayManager"):
		ui_display_manager = get_node("UiDisplayManager")
		print("Avatar: Found UiDisplayManager: ", ui_display_manager)

	# Add FloatingScreenManager
	var floating_manager = load("res://apps/3dsim/managers/floating_screen_manager.gd").new()
	floating_manager.name = "FloatingScreenManager"
	add_child(floating_manager)

#-----------------------------------------------------

# Add these new constants
#===============================================================================
# NFT SYSTEM - NFT sphere spawning and popup management
#===============================================================================

# Preload NFT-related scenes
const NFT_SPHERE_SCENE = preload("res://core/facilities/nft-sphere.tscn")
const POPUP_SCENE = preload("res://core/widgets/nft-create-popup.tscn")

# Active popup tracking
var active_popup: Control = null

## Spawns an NFT sphere in the world (synchronized via RPC)
@rpc("any_peer", "call_local")
func spawn_nft_sphere(nft_data: Dictionary, spawn_position: Vector3):
	var nft_sphere = NFT_SPHERE_SCENE.instantiate()
	nft_sphere.set_nft_data(nft_data)
	%Universe.add_child(nft_sphere)
	nft_sphere.global_transform.origin = spawn_position + Vector3(0, 1, 0)  # Offset slightly above the ground
	print("Spawned NFT sphere at position: ", nft_sphere.global_transform.origin)
	print("NFT data set: ", nft_data)  # Debug print

# Modify the _on_nft_issued function to use RPC
func _on_nft_issued(nft_data, spawn_position: Vector3):
	print("NFT issued with data: ", nft_data)  # Debug print
	if multiplayer.is_server():
		spawn_nft_sphere.rpc(nft_data, spawn_position)
	else:
		# Send to server for validation and distribution
		spawn_nft_sphere.rpc_id(1, nft_data, spawn_position)
	active_popup.queue_free()
	active_popup = null  # Clear the active popup reference

# Add this new function
func handle_click(event_position: Vector2):
	# First, check if we clicked on the existing popup
	if active_popup and active_popup.get_global_rect().has_point(event_position):
		return  # Ignore clicks on the popup itself

	if camera:
		var from = camera.project_ray_origin(event_position)
		var to = from + camera.project_ray_normal(event_position) * RAY_LENGTH
		var result = do_raycast_nft(from, to)
		
		if result and result.collider is StaticBody3D:
			if Input.is_key_pressed(KEY_CTRL):  # Check if Ctrl is pressed
				if not active_popup:  # Only create a new popup if one doesn't exist
					show_nft_popup(result.position)

			# if Profile.wallet != "":  # Assuming you have a Global singleton to check login status
			# 	if not active_popup:  # Only create a new popup if one doesn't exist
			# 		show_nft_popup(result.position)
			# else:
			# 	print("Please log in with Web3 wallet first")

# Add this new function (renamed from do_raycast to do_raycast_nft)
func do_raycast_nft(from: Vector3, to: Vector3):        
	var space_state = %Universe.get_world_3d().direct_space_state
	
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	return space_state.intersect_ray(query)

# Add these new functions
func show_nft_popup(spawn_position: Vector3):
	active_popup = POPUP_SCENE.instantiate()
	add_child(active_popup)

	active_popup.connect("nft_issued", Callable(self, "_on_nft_issued").bind(spawn_position))
	active_popup.connect("tree_exited", Callable(self, "_on_popup_closed"))

func _on_popup_closed():
	active_popup = null  # Clear the active popup reference when it's closed

#===============================================================================
# INPUT HANDLING - Keyboard shortcuts and input coordination
#===============================================================================

## Main input handler - coordinates all input systems
## Routes keyboard shortcuts and manages input adapter states
func _input(event):
	# Only handle UI-related input if a display is active
	if ui_display_manager and ui_display_manager.is_display_active():
		# Handle mouse click outside of displays to close them
		if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and event.pressed:
			# Check if this is a click outside the active display
			if ui_display_manager.get_active_display() == "modelica":
				if !_is_click_on_modelica_display(event.position):
					# Close the display and continue with regular input
					ui_display_manager.close_modelica_display()
					# Don't mark as handled so the click can still be processed
					return
		
		# Handle keyboard events for displays
		if event is InputEventKey:
			var handled = ui_display_manager.process_key_event(event)
			if handled:
				# For the console, we want to block avatar's internal shortcuts
				# BUT we must let the event reach the LineEdit focus phase.
				# In Godot, marking handled here BLOCKS the LineEdit from receiving it.
				if ui_display_manager.get_active_display() != "console" or event.keycode == KEY_ESCAPE:
					get_viewport().set_input_as_handled()
				return
			
			# Special case for Escape key to close ModelicaUI
			if event.pressed and event.keycode == KEY_ESCAPE:
				if ui_display_manager.get_active_display() == "modelica":
					print("Avatar: Handling Escape key to close ModelicaUI")
					ui_display_manager.close_modelica_display()
					get_viewport().set_input_as_handled()
					return
		
		# Handle mouse events for displays
		if (event is InputEventMouseButton or event is InputEventMouseMotion) and !event.is_echo():
			var handled = ui_display_manager.process_mouse_event(event)
			if handled:
				get_viewport().set_input_as_handled()
				return
	
	# Continue with regular avatar input if UI hasn't handled it
	if active_popup:
		return  # Ignore input when popup is active

	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and event.pressed:
		handle_click(event.position)

	if Input.is_action_just_pressed("main_menu"): #TBD maybe move from avatar?
		#SceneManager.no_effect_change_scene("back")
		#TBD: Show/hide menu, should be a signal? To what?
		LCWindows.toggle_main_menu()
	
	if Input.is_action_just_pressed("ui_focus_next"):
		LCWindows.toggle_chat()
	
	if event is InputEventKey and not event.is_echo() and event.is_pressed():
		# Process display toggle keys if UiDisplayManager exists
		if ui_display_manager:
			if event.keycode == KEY_TAB:
				ui_display_manager.toggle_supply_chain_display()
				return
		
		var key_number: int = -1
		
		# Process number keys for entity control
		match event.keycode:
			Key.KEY_1:
				key_number = 1
			Key.KEY_2:
				key_number = 2
			Key.KEY_3:
				key_number = 3
			Key.KEY_4:
				key_number = 4
			Key.KEY_5:
				key_number = 5
			Key.KEY_6:
				key_number = 6
			Key.KEY_7:
				key_number = 7
			Key.KEY_8:
				key_number = 8
			Key.KEY_BACKSPACE: #TBD: Move to actions
				request_release_control()
				return
				
		if key_number != -1:
			if event.is_alt_pressed():
				spawn_entity.emit(key_number-1)
			else:
				requesting_control.emit(key_number-1)
		
	# Handle camera/movement controls when no UI display is active 
	# or when UI is visible but not active
	if ui_display_manager == null or not ui_display_manager.is_display_active():
		# Enable/disable avatar input adapter based on whether we have a target
		# When target is null, avatar can move freely
		# When target is set, avatar is controlling an entity
		if has_node("AvatarInputAdapter"):
			$AvatarInputAdapter.set_process(target == null)
		
		# Always update camera basis for avatar controller
		if camera and has_node("AvatarController"):
			$AvatarController.camera_basis = camera.get_camera_rotation_basis()
		
	input_camera(event)

#===============================================================================
# CAMERA INPUT - Camera rotation and zoom controls
#===============================================================================

## Handles camera rotation (mouse/keyboard) and zoom (wheel/keyboard)
## Manages mouse capture mode for camera control
func input_camera(event):
	# Rotating camera
	if Input.is_action_pressed("rotate_camera"):
		Input.set_mouse_mode(Input.MOUSE_MODE_CAPTURED)
		mouse_control = true
	else:
		Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
		mouse_control = false
		
	# Processing input related to camera
	if camera is SpringArmCamera:
		var cam: SpringArmCamera = camera
		var camera_move := Vector2.ZERO

		# Code responsible for camera movement. Better update to action
		if (event is InputEventMouseMotion) and mouse_control:
			camera_move = event.relative * MOUSE_SENSITIVITY
		
		camera_move += Vector2(
			Input.get_action_strength("camera_left") - Input.get_action_strength("camera_right"),
			Input.get_action_strength("camera_up") - Input.get_action_strength("camera_down")
		)
		cam.rotate_relative(camera_move) #-> For certain controllers new camera direction matters
		
		# Manupulations with camera. TBD to actions as well
		var delta_camera_spring_length = Input.get_action_strength("plus") - Input.get_action_strength("minus")

		if event is InputEventMouseButton:
			if event.button_index == MOUSE_BUTTON_WHEEL_UP:
				delta_camera_spring_length -= WHEEL_ZOOM_INCREMENT
			elif event.button_index == MOUSE_BUTTON_WHEEL_DOWN:
				delta_camera_spring_length += WHEEL_ZOOM_INCREMENT
				
		# Use inc_spring_length or modify SPRING_LENGTH instead of accessing spring_length directly
		if delta_camera_spring_length != 0:
			cam.inc_spring_length(delta_camera_spring_length * ZOOM_SPEED)

func input_operator(event):
	if target is LCOperatorController:
		var operator: LCOperatorController = target
		operator.orient(camera.get_plain_basis())
		
#===============================================================================
# RAYCASTING & INTERACTION - Click-to-select and world interaction
#===============================================================================

## Performs raycast from screen position for entity selection or spawning
func action_raycast(_position: Vector2):
	if camera:  
		var from = camera.project_ray_origin(_position)
		var to = from + camera.project_ray_normal(_position) * RAY_LENGTH
		do_raycast(from, to)

func do_raycast(from: Vector3, to: Vector3):		
	var space_state = %Universe.get_world_3d().direct_space_state
	
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.exclude = [self]
	var result = space_state.intersect_ray(query)
	
	if result:
		#TBD Should be via tools, 3 tools: TargetTool, SelectionTool, SpawnTool
		#TBD Could be adding to selection
		if result.collider is StaticBody3D:
			spawn_entity.emit(entity_to_spawn, result.position + Vector3(0, 1, 0))
		else:
			requesting_control.emit(result.collider)
#------------------------------------------------------
# Function _on_state_transited instantiates different ui based on target and sets camera spring length
#===============================================================================
# STATE TRANSITIONS - Camera settings per controller type
#===============================================================================

## Called when target changes - applies appropriate camera settings
## Avatar owns visualization preferences for each controller type
func _on_state_transited():

	camera.set_follow_height(0.5)
	camera.set_spring_length(2.5)
	
	if target is LCCharacterController:
		camera.set_spring_length(2.5)
		camera.set_follow_height(2.0)
	elif target is LCSpacecraftController:
		camera.set_spring_length(50)
		camera.set_follow_height(0)
	elif target is LCRoverController:
		camera.set_spring_length(5) 
		camera.set_follow_height(1.5)
	elif target is LCOperatorController:
		camera.set_spring_length(2.5)

	target_changed.emit(target)
	
	ui.set_target(target)

	if camera != null:
		camera.target = target

# Entities are tracked by simulation
# Avatar delegates entity list updates to UI
# This method is called by simulation when entities are updated
func update_entities(entities):
	if ui:
		ui.update_entities(entities)

func camera_global_position():
	return camera.global_position

#===============================================================================
# SIGNAL HANDLERS - Responding to control events and user actions
#===============================================================================

## Handles entity spawning requests from UI
func _on_select_entity_to_spawn(entity_id=0, spawn_position=null):
	if is_multiplayer_authority():
		get_parent().spawn.rpc_id(1, entity_id, spawn_position)
	else:
		get_parent().spawn.rpc_id(1, entity_id, spawn_position)

func _on_existing_entity_selected(idx):
	print("Avatar: Requesting control for entity index: ", idx)
	get_parent().request_control_by_index(idx)

func request_release_control():
	if target:
		get_parent()._on_avatar_release_control(target.get_path())
		set_target(null)

func _on_simulation_control_granted(path):
	print("Avatar: Control granted for entity: ", path)
	var entity = get_node(path)
	if entity:
		print("Avatar: Setting target to ", entity.name)
		set_target(entity)
	else:
		print("Avatar: Failed to get node for path: ", path)

func _on_simulation_control_declined(path):
	print("Avatar: Control declined for entity: ", path)

func _on_simulation_control_released(path):
	print("Avatar: Control released for entity: ", path)

func _on_select_entity_to_control(entity):
	if entity is Node:  # Ensure entity is a Node
		ControlManager.request_control(entity.get_path())
	else:
		print("Error: entity is not a Node")

func _on_release_control(entity):
	if entity is Node:  # Ensure entity is a Node
		ControlManager.release_control(entity.get_path())
	else:
		print("Error: entity is not a Node")

func _on_control_granted(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		print("Avatar: Control granted for entity: ", entity_path)
		var entity = get_node(entity_path)
		set_target(entity)
		# Update UI or other necessary changes

func _on_control_request_denied(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		print("Avatar: Control denied for entity: ", entity_path)
		# Update UI or show a message to the user

# Helper method to check if a click position intersects with the ModelicaUI display
func _is_click_on_modelica_display(click_position: Vector2) -> bool:
	if !camera:
		return false
		
	var from = camera.project_ray_origin(click_position)
	var to = from + camera.project_ray_normal(click_position) * RAY_LENGTH
	
	var space_state = get_world_3d().direct_space_state
	var query = PhysicsRayQueryParameters3D.create(from, to)
	query.collide_with_areas = true
	query.collision_mask = 2  # Make sure this matches the collision layer of the ModelicaUI Area3D
	
	var result = space_state.intersect_ray(query)
	if result and result.collider:
		# Check if the collider is part of the ModelicaUI
		var node = result.collider
		if node.is_in_group("modelica_display") or node.get_parent().is_in_group("modelica_display"):
			return true
	
	return false

func get_command_metadata() -> Dictionary:
	return {
		"TAKE_CONTROL": {
			"description": "Take control of an entity by name or index.",
			"arguments": {
				"target": {
					"type": "string",
					"description": "Entity name or index."
				}
			}
		},
		"STOP_CONTROL": {
			"description": "Release control of the current entity."
		},
		"KEY_DOWN": {
			"description": "Send a key down event.",
			"arguments": {
				"key": {
					"type": "enum",
					"values": ["w", "s", "a", "d", "q", "e", "space", "shift", "v", "f"],
					"description": "The key to press down."
				}
			}
		},
		"KEY_UP": {
			"description": "Send a key up event.",
			"arguments": {
				"key": {
					"type": "enum",
					"values": ["w", "s", "a", "d", "q", "e", "space", "shift", "v", "f"],
					"description": "The key to release."
				}
			}
		},
		"KEY_PRESS": {
			"description": "Send a short key press (down then up).",
			"arguments": {
				"key": {
					"type": "enum",
					"values": ["w", "s", "a", "d", "q", "e", "space", "shift", "v", "f"],
					"description": "The key to press."
				}
			}
		}
	}

#===============================================================================
# REMOTE CONTROL COMMANDS - Commands for remote execution
#===============================================================================

func cmd_take_control(target: Variant) -> String:
	if target == null:
		return "Missing target argument"
		
	if target is float or target is int:
		_on_existing_entity_selected(int(target))
		return "Requested control for entity index: %d" % int(target)
	elif target is String:
		# Try to find node by name or path
		var target_node = get_tree().root.find_child(target, true, false)
		if target_node:
			ControlManager.request_control(target_node.get_path(), multiplayer.get_unique_id())
			return "Requested control for node: %s" % target_node.name
		else:
			return "Entity not found: %s" % target
			
	return "Invalid target type"

func cmd_stop_control() -> String:
	request_release_control()
	return "Control released"

# Track keys held down by remote console to re-apply them every frame
var held_remote_keys = {}

func _physics_process(_delta):
	# Re-apply held keys state aggressively every physics frame.
	# We use parse_input_event to trigger _input/unhandled_input logic 
	# and action_press to update polling-based systems.
	# This helps overcome Focus Loss resets in Godot.
	for key_str in held_remote_keys:
		var keycode = held_remote_keys[key_str]
		var event = InputEventKey.new()
		event.device = DEVICE_ID_REMOTE
		event.pressed = true
		event.keycode = keycode
		event.physical_keycode = keycode
		event.echo = true # Mark as echo to show it's continuous
		
		# Update global action state
		for action in InputMap.get_actions():
			if InputMap.event_is_action(event, action):
				Input.action_press(action, 1.0)
		
		# Trigger event-based input processing
		Input.parse_input_event(event)

func cmd_key_down(key: String) -> String:
	# Mark as remote to bypass UI capture
	var result = _mimic_key(key, true)
	
	# Store in held keys if successful
	if not result.begins_with("Unknown"):
		# Extract keycode from result or find again
		var keycode = OS.find_keycode_from_string(key)
		if keycode == KEY_NONE: keycode = OS.find_keycode_from_string(key.to_upper())
		if keycode != KEY_NONE:
			held_remote_keys[key.to_lower()] = keycode
			
	return result

func cmd_key_up(key: String) -> String:
	held_remote_keys.erase(key.to_lower())
	return _mimic_key(key, false)

func get_held_remote_keys() -> Dictionary:
	return held_remote_keys

func cmd_key_press(key: String) -> String:
	_mimic_key(key, true)
	# Add a small delay for the release event so the game logic has time to detect the press
	get_tree().create_timer(0.1).timeout.connect(func(): 
		held_remote_keys.erase(key.to_lower()) # Also ensure it's removed from held
		_mimic_key(key, false)
	)
	return "Key pressed: %s" % key

func _mimic_key(key_str: String, pressed: bool) -> String:
	if key_str == "":
		return "Missing key argument"
		
	# OS.find_keycode_from_string is available in most Godot 4 versions 
	# as an alternative to the newer DisplayServer method.
	var keycode = KEY_NONE
	if OS.has_method("find_keycode_from_string"):
		keycode = OS.find_keycode_from_string(key_str)
		if keycode == KEY_NONE:
			keycode = OS.find_keycode_from_string(key_str.to_upper())
			
	if keycode == KEY_NONE:
		return "Unknown key: %s" % key_str
		
	var event = InputEventKey.new()
	event.device = DEVICE_ID_REMOTE
	event.pressed = pressed
	event.keycode = keycode
	event.physical_keycode = keycode
	event.echo = false
	
	# Detect actions for this key for debug and aggressive state management
	var matched_actions = []
	for action in InputMap.get_actions():
		if InputMap.event_is_action(event, action):
			matched_actions.append(action)
			if pressed:
				Input.action_press(action, 1.0) # Explicitly set strength to 1.0
			else:
				Input.action_release(action)
	
	if pressed:
		print("DEBUG: _mimic_key key=%s pressed=%s keycode=%d actions=%s" % [key_str, pressed, keycode, matched_actions])
	
	# Pass to global input system to trigger _input/unhandled_input callbacks
	Input.parse_input_event(event)
				
	return "Key %s: %s (code: %d, actions: %s)" % ["down" if pressed else "up", key_str, keycode, matched_actions]

static func is_remote_event(event: InputEvent) -> bool:
	return event != null and event.device == DEVICE_ID_REMOTE
