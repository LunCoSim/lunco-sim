extends Control

# Preload all controller UI scenes at compile time for better performance
const CHARACTER_UI = preload("res://controllers/character/character-ui.tscn")
const SPACECRAFT_UI = preload("res://controllers/spacecraft/spacecraft-ui.tscn")
const ROVER_UI = preload("res://controllers/rover/rover-ui.tscn")
const ROVER_JOINT_UI = preload("res://controllers/rover/rover-joint-ui.tscn")
const OPERATOR_UI = preload("res://controllers/operator/operator-ui.tscn")

# Dictionary for cleaner lookup by controller class name
const UI_SCENES = {
	"LCCharacterController": CHARACTER_UI,
	"LCSpacecraftController": SPACECRAFT_UI,
	"LCRoverController": ROVER_UI,
	"LCOperatorController": OPERATOR_UI
}

signal entity_selected(int)
signal existing_entity_selected(int)

@onready var ui := %TargetUI
@onready var ui_helper = get_node("/root/UIHelper")
@onready var physio_network_bridge = $PhysioNetworkBridge

var controller_ui: Node
var avatar: LCAvatar
# Called when the node enters the scene tree for the first time.
func _ready():
	print("Avatar UI is initializing")
	
	# Check for essential nodes
	if get_node_or_null("%LiveEntities") == null:
		push_error("Avatar UI: LiveEntities node not found")
	
	if get_node_or_null("%TargetUI") == null:
		push_error("Avatar UI: TargetUI node not found")

	# Setup buttons FIRST to ensure they work even if other things fail
	print("Avatar UI: Setting up buttons...")
	var create_button = get_node_or_null("%CreateEntityButton")
	if create_button:
		print("Avatar UI: CreateEntityButton found")
		if not create_button.pressed.is_connected(_on_create_entity_button_pressed):
			create_button.pressed.connect(_on_create_entity_button_pressed)
			
		if ui_helper:
			ui_helper.setup_entity_button(create_button, false)
		# Apply color AFTER setup to ensure it overrides defaults
		create_button.modulate = Color(0.2, 0.8, 0.2)
		create_button.focus_mode = Control.FOCUS_NONE
	else:
		push_error("Avatar UI: CreateEntityButton not found")

	var builder_button = get_node_or_null("%BuilderButton")
	if builder_button:
		print("Avatar UI: BuilderButton found")
		if not builder_button.pressed.is_connected(_on_builder_button_pressed):
			builder_button.pressed.connect(_on_builder_button_pressed)
			
		if ui_helper:
			ui_helper.setup_entity_button(builder_button, false)
		builder_button.modulate = Color(0.2, 0.6, 1.0) # Blue-ish
		builder_button.focus_mode = Control.FOCUS_NONE
	else:
		push_warning("Avatar UI: BuilderButton not found")

	var inspector_button = get_node_or_null("%InspectorButton")
	if inspector_button:
		print("Avatar UI: InspectorButton found")
		if not inspector_button.pressed.is_connected(_on_inspector_button_pressed):
			inspector_button.pressed.connect(_on_inspector_button_pressed)
			
		if ui_helper:
			ui_helper.setup_entity_button(inspector_button, false)
		inspector_button.modulate = Color(0.8, 0.4, 0.8) # Purple-ish
		inspector_button.focus_mode = Control.FOCUS_NONE
	else:
		push_warning("Avatar UI: InspectorButton not found")
		
	var effector_button = get_node_or_null("%EffectorButton")
	if effector_button:
		print("Avatar UI: EffectorButton found")
		if not effector_button.pressed.is_connected(_on_effector_button_pressed):
			effector_button.pressed.connect(_on_effector_button_pressed)
			
		if ui_helper:
			ui_helper.setup_entity_button(effector_button, false)
		effector_button.modulate = Color(1.0, 0.6, 0.2) # Orange-ish
		effector_button.focus_mode = Control.FOCUS_NONE
	else:
		push_warning("Avatar UI: EffectorButton not found")
	
	# Connect visibility change signal to update entities when UI becomes visible
	visibility_changed.connect(_on_visibility_changed)
	
	# Populate entity creation list
	var entities_list = get_node_or_null("%Entities")
	if entities_list:
		entities_list.clear()
		if EntitiesDB:
			for entity_key in EntitiesDB.Entities.keys():
				entities_list.add_item(entity_key)
	
	avatar = get_parent()
	if avatar:
		existing_entity_selected.connect(avatar._on_existing_entity_selected)
		entity_selected.connect(avatar._on_select_entity_to_spawn)
		
		var tree: ItemList = get_node_or_null("%Entities")
		if tree and avatar.entity_to_spawn != null:
			# Make sure entity_to_spawn is a valid index
			if typeof(avatar.entity_to_spawn) == TYPE_INT:
				if avatar.entity_to_spawn >= 0 and avatar.entity_to_spawn < tree.item_count:
					tree.select(avatar.entity_to_spawn)
		
		# Try to update entities list if we can access them
		if avatar.get_parent() and "entities" in avatar.get_parent():
			update_entities(avatar.get_parent().entities)
	else:
		push_error("Avatar UI: Parent avatar not found")
	
	# Connect signals
	if Users:
		Users.users_updated.connect(_on_update_connected_users)
	
	if Profile:
		Profile.profile_changed.connect(_on_profile_changed)
	
	# Connect to networking signals for connection status
	if LCNet:
		LCNet.connection_state_changed.connect(_on_connection_state_changed)
	
	_update_connection_status()
	
	# Update user list
	if Users:
		_on_update_connected_users()
	
	# Update server connection status
	_on_connection_state_changed(LCNet.connection_state if LCNet else "disconnected")
	
	print("Avatar UI: Ready complete")


func _input(event):
	# Check if input is captured by other displays (like console)
	if avatar and avatar.ui_display_manager and avatar.ui_display_manager.is_input_captured():
		return
		
	# Toggle component inspector & settings with 'I' key
	if event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_I:
			var inspector = get_node_or_null("ComponentInspector")
			if inspector:
				inspector.visible = not inspector.visible
				print("Component Inspector toggled: ", inspector.visible)

# Function display_controller_ui clears the ui and displays the controller UI
func display_controller_ui(new_controller_ui: Node = null):
	clear_ui()
	if new_controller_ui and ui:
		ui.add_child(new_controller_ui)
		ui.visible = true
	else:
		ui.visible = false

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass




# Function clear_ui removes child items if ui exists	
func clear_ui():
	if ui:
		for n in ui.get_children():
			ui.remove_child(n)
		ui.visible = false

func set_target(target):
	clear_ui()
	
	# Use type checking instead of string lookup
	if target:
		var ui_scene = null
		
		if target is LCRoverController:
			ui_scene = ROVER_UI
		elif target is LCRoverJointController:
			ui_scene = ROVER_JOINT_UI
		elif target is LCCharacterController:
			ui_scene = CHARACTER_UI
		elif target is LCSpacecraftController:
			ui_scene = SPACECRAFT_UI
		elif target is LCOperatorController:
			ui_scene = OPERATOR_UI
		
		if ui_scene:
			controller_ui = ui_scene.instantiate()
			controller_ui.set_target(target)  # controller specific function
			display_controller_ui(controller_ui)
		else:
			var class_name_str = "Unknown"
			if target.get_script():
				class_name_str = target.get_script().resource_path.get_file()
			push_warning("Avatar UI: No UI scene found for controller: " + str(target) + " (" + class_name_str + ")")
	
	# Notify BuilderManager to select this entity for inspection
	# This ensures the Inspector updates when we take control
	if target:
		var entity = target
		if target is LCController:
			entity = target.get_parent()
		
		if BuilderManager:
			BuilderManager.select_entity(entity)
	
	# Only update entities if we're attached to the parent properly
	var parent = get_parent()
	if parent and parent.get_parent() and "entities" in parent.get_parent():
		# Use call_deferred to ensure the scene tree is ready
		call_deferred("update_entities", parent.get_parent().entities)

func _on_entities_item_selected(index):
	print("_on_entities_item_selected: ", index)
	if avatar:
		avatar.entity_to_spawn = index
	emit_signal("entity_selected", index)

func _on_existing_entity_selected(idx):
	print("DEBUG: UI button clicked for entity: ", idx)
	existing_entity_selected.emit(idx)
	
func update_entities(entities):
	print("UI: Updating entities list with ", entities.size(), " entities")
	var tree = get_node_or_null("%LiveEntities")
	
	if tree == null:
		push_error("LiveEntities node not found")
		return
		
	for child in tree.get_children():
		child.queue_free()
	
	var idx = 0
	
	# Adjust columns based on entity count for better layout
	# if tree is GridContainer:
	# 	if entities.size() <= 8:
	# 		tree.columns = 8
	# 	elif entities.size() <= 16:
	# 		tree.columns = 8
	# 	else:
	# 		tree.columns = 10
	
	for i in range(entities.size()):
		var entity = entities[i]
		if not is_instance_valid(entity):
			continue
			
		var entity_name = str(entity.name)
		var button = Button.new()
		button.text = entity_name
		
		# Shorten the entity name for better display
		entity_name = ui_helper.format_entity_name(entity_name)
		
		# Check if the entity has a multiplayer authority
		var owner_id = entity.get_multiplayer_authority()
		if owner_id != 1:  # Skip server authority
			entity_name += "\n#" + str(owner_id)
		
		print("UI: Creating button for entity ", entity_name, " at index ", idx)
		button.text = entity_name
		
		# Use theme type variation and set up button
		var is_active = false
		if avatar.target:
			# Check if the entity itself is the target
			if entity == avatar.target:
				is_active = true
			# Check if the entity is the parent of the target (if target is a controller)
			elif entity == avatar.target.get_parent():
				is_active = true
			# Check if the target is a controller of the entity (using find_controller logic)
			elif LCController.find_controller(entity) == avatar.target:
				is_active = true
				
		ui_helper.setup_entity_button(button, is_active)
		button.tooltip_text = str(entity.name) + " (Owner: " + str(owner_id) + ")"
		
		tree.add_child(button)
		# Connect with BINDING to ensure the correct index is passed
		button.pressed.connect(_on_existing_entity_selected.bind(idx))
		print("UI: Connected button for entity ", entity_name, " to index ", idx)
		
		idx += 1

func _on_create_entity_button_pressed():
	print("DEBUG: Create Entity button pressed!")
	var button_node = get_node_or_null("%CreateEntityButton")
	if not button_node:
		push_error("CreateEntityButton node not found in callback")
		return
		
	var popup = PopupMenu.new()
	popup.name = "CreateEntityPopup"
	add_child(popup)
	
	if EntitiesDB:
		print("Populating entities from DB")
		var keys = EntitiesDB.Entities.keys()
		for key in keys:
			var value = EntitiesDB.Entities[key]
			popup.add_item(key, value)
	else:
		push_error("EntitiesDB singleton not found")
	
	popup.id_pressed.connect(func(id): 
		_on_entities_item_selected(id)
		popup.queue_free()
	)
	popup.popup_hide.connect(func(): popup.queue_free())
	
	# Show popup near the button
	var rect = button_node.get_global_rect()
	popup.position = Vector2(rect.position.x, rect.position.y - popup.size.y) # Show above if possible, or let it handle itself
	popup.popup(Rect2(rect.position.x, rect.position.y, rect.size.x, rect.size.y))

func _on_builder_button_pressed():
	print("DEBUG: Builder button pressed!")
	
	# Get the LeftSideContainer
	var left_container = get_node_or_null("LeftSideContainer")
	if not left_container:
		push_error("LeftSideContainer not found")
		return
	
	# Check if builder UI already exists
	var existing_builder = left_container.get_node_or_null("BuilderUI")
	if existing_builder:
		existing_builder.visible = not existing_builder.visible
		print("Builder UI toggled: ", existing_builder.visible)
		return

	var builder_ui_scene = load("res://core/ui/builder_ui.tscn")
	if builder_ui_scene:
		print("Builder UI scene loaded")
		var builder_ui = builder_ui_scene.instantiate()
		builder_ui.name = "BuilderUI" # Ensure consistent name
		builder_ui.size_flags_vertical = Control.SIZE_EXPAND_FILL # Fill available space
		left_container.add_child(builder_ui)
		
		# Check if BuilderManager exists (it should be an autoload)
		if has_node("/root/BuilderManager"):
			print("Starting building mode")
			get_node("/root/BuilderManager").start_building()
		else:
			push_error("BuilderManager singleton not found! Please restart the project.")
	else:
		push_error("Failed to load Builder UI scene")

func _on_inspector_button_pressed():
	print("DEBUG: Inspector & Settings button pressed!")
	var inspector = get_node_or_null("ComponentInspector")
	if inspector:
		inspector.visible = not inspector.visible
		print("Component Inspector toggled: ", inspector.visible)
	else:
		push_error("ComponentInspector node not found")

func _on_effector_button_pressed():
	print("DEBUG: Effector button pressed!")
	var inspector = get_node_or_null("EffectorInspector")
	if inspector:
		if inspector.visible:
			inspector.hide()
			print("Effector Inspector hidden")
		else:
			inspector.popup_centered()
			print("Effector Inspector shown")
	else:
		push_error("EffectorInspector node not found")

func _on_update_connected_users():
	var tree: ItemList = get_node_or_null("%Users")
	if tree == null:
		push_error("Avatar UI: Users node not found")
		return
	
	tree.clear()
	if Users:
		for user_id in Users.users:
			var username = Users.users[user_id]["username"]

			if username == "":
				username = "Unknown"
				
			tree.add_item(username)

func select_entity(idx):
	var tree = get_node_or_null("%LiveEntities")
	
	if tree == null:
		push_error("Avatar UI: LiveEntities node not found in select_entity")
		return
	
	for child in tree.get_children():
		if child is Button:
			child.flat = false
			if child.get_index() == idx:
				child.flat = true

func _on_profile_changed():
	_update_connection_status()

func _update_connection_status():
	var connect_wallet = get_node_or_null("%ConnectWallet")
	var disconnect_wallet = get_node_or_null("%DisconnectWallet")
	var wallet_info = get_node_or_null("%WalletInfoLabel")
	var profile_nft = get_node_or_null("%ProfileNFT")
	var gitcoin_donor = get_node_or_null("%GitcoinDonor")
	var artizen_buyer = get_node_or_null("%ArtizenBuyer")
	
	if Profile == null:
		push_error("Avatar UI: Profile singleton not found")
		return
		
	if connect_wallet:
		connect_wallet.visible = Profile.wallet == ""
	
	if disconnect_wallet:
		disconnect_wallet.visible = Profile.wallet != ""

	if wallet_info:
		wallet_info.text = ("Connected: " + Profile.wallet.substr(0, 8) + "...") if Profile.wallet != "" else "Not connected"
	
	if profile_nft:
		profile_nft.text = "Yes" if Profile.has_profile > 0 else "No"
	
	if gitcoin_donor:
		gitcoin_donor.text = "Yes" if Profile.is_donor() else "No"
	
	if artizen_buyer:
		artizen_buyer.text = "Yes" if Profile.is_artizen_buyer else "No"

func _on_connect_wallet_pressed():
	Profile.login()

func _on_disconnect_wallet_pressed():
	Profile.logout()

func _on_control_granted(peer_id: int, entity_path: NodePath):
	# Always update entities to reflect the new owner in the UI
	if get_parent() and get_parent().get_parent() and "entities" in get_parent().get_parent():
		update_entities(get_parent().get_parent().entities)

func _on_control_request_denied(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		# Maybe show a message to the user
		print("Control request denied for entity: ", entity_path)

func _on_control_released(peer_id: int, entity_path: NodePath):
	# Always update entities to reflect the released control
	if get_parent() and get_parent().get_parent() and "entities" in get_parent().get_parent():
		update_entities(get_parent().get_parent().entities)

func _on_visibility_changed():
	# Update entity list when the UI becomes visible
	if visible:
		var parent = get_parent()
		if parent and parent.get_parent() and parent.get_parent().has_method("get_entities"):
			call_deferred("update_entities", parent.get_parent().get_entities())

func _on_PhysioNetworkBridgeButton_pressed():
	physio_network_bridge.visible = !physio_network_bridge.visible
		if parent and parent.get_parent() and "entities" in parent.get_parent():
			call_deferred("update_entities", parent.get_parent().entities)

func _on_menu_button_pressed():
	print("Menu button pressed")
	# Toggle the main menu
	if LCWindows:
		LCWindows.toggle_main_menu()
	else:
		push_error("LCWindows singleton not found")

func _on_connection_state_changed(state: String):
	var status_label = get_node_or_null("%ConnectionStatus")
	if status_label == null:
		return
	
	# Update the connection status label based on state
	match state:
		"disconnected":
			status_label.text = "Server: Disconnected"
			status_label.modulate = Color(0.7, 0.7, 0.75, 1)
		"connecting":
			status_label.text = "Server: Connecting..."
			status_label.modulate = Color(0.9, 0.9, 0.5, 1)
		"connected":
			status_label.text = "Server: Connected"
			status_label.modulate = Color(0.5, 0.9, 0.5, 1)
		"failed":
			status_label.text = "Server: Failed"
			status_label.modulate = Color(0.9, 0.5, 0.5, 1)
		_:
			status_label.text = "Server: " + state
			status_label.modulate = Color(0.7, 0.7, 0.75, 1)
