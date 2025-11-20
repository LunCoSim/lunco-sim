extends Control

signal entity_selected(int)
signal existing_entity_selected(int)

@onready var ui := %TargetUI
@onready var ui_helper = get_node("/root/UIHelper")

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
		
		var tree: ItemList = get_node_or_null("%Entities")
		if tree and avatar.entity_to_spawn != null:
			# Make sure entity_to_spawn is a valid index
			if typeof(avatar.entity_to_spawn) == TYPE_INT:
				if avatar.entity_to_spawn >= 0 and avatar.entity_to_spawn < tree.item_count:
					tree.select(avatar.entity_to_spawn)
		
		# Try to update entities list if we can access them
		if avatar.get_parent() and avatar.get_parent().has_method("get_entities"):
			update_entities(avatar.get_parent().get_entities())
	else:
		push_error("Avatar UI: Parent avatar not found")
	
	# Connect signals
	if Users:
		Users.users_updated.connect(_on_update_connected_users)
	
	if Profile:
		Profile.profile_changed.connect(_on_profile_changed)
	
	_update_connection_status()
	
	# Add this line to update the user list when the scene is ready
	_on_update_connected_users()

# Function display_controller_ui clears the ui and displays the controller UI
func display_controller_ui(new_controller_ui: Node = null):
	clear_ui()
	if new_controller_ui and ui:
		ui.add_child(new_controller_ui)

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass




# Function clear_ui removes child items if ui exists	
func clear_ui():
	if ui:
		for n in ui.get_children():
			ui.remove_child(n)

func set_target(target):
	clear_ui()
	
	if target is LCCharacterController:
		controller_ui = load("res://controllers/character/character-ui.tscn").instantiate()
	elif target is LCSpacecraftController:
		controller_ui = load("res://controllers/spacecraft/spacecraft-ui.tscn").instantiate()
	elif target is LCRoverController:
		controller_ui = load("res://controllers/rover/rover-ui.tscn").instantiate()
	elif target is LCOperatorController:
		controller_ui = load("res://controllers/operator/operator-ui.tscn").instantiate()

	if controller_ui:
		controller_ui.set_target(target) #controller specific function
	display_controller_ui(controller_ui)
	
	# Only update entities if we're attached to the parent properly
	var parent = get_parent()
	if parent and parent.get_parent() and parent.get_parent().has_method("get_entities"):
		# Use call_deferred to ensure the scene tree is ready
		call_deferred("update_entities", parent.get_parent().get_entities())

func _on_entities_item_selected(index):
	print("_on_entities_item_selected: ", index)
	emit_signal("entity_selected", index)
	pass # Replace with function body.

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
	if tree is GridContainer:
		if entities.size() <= 8:
			tree.columns = 8
		elif entities.size() <= 16:
			tree.columns = 8
		else:
			tree.columns = 10
	
	for entity in entities:
		# Add child items to the root.
		var button = Button.new()
		var entity_name = str(entity.name)
		
		# Shorten the entity name for better display
		entity_name = ui_helper.format_entity_name(entity_name)
		
		# Check if the entity has a multiplayer authority
		var owner_id = entity.get_multiplayer_authority()
		if owner_id != 1:  # Skip server authority
			entity_name += "\n#" + str(owner_id)
		
		print("UI: Creating button for entity ", entity_name, " at index ", idx)
		button.text = entity_name
		
		# Use theme type variation and set up button
		var is_active = avatar.target and entity == avatar.target.get_parent()
		ui_helper.setup_entity_button(button, is_active)
		button.tooltip_text = str(entity.name) + " (Owner: " + str(owner_id) + ")"
		
		tree.add_child(button)
		button.pressed.connect(_on_existing_entity_selected.bind(idx))
		
		idx += 1

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
	if peer_id == multiplayer.get_unique_id():
		update_entities(get_parent().get_parent().entities)

func _on_control_request_denied(peer_id: int, entity_path: NodePath):
	if peer_id == multiplayer.get_unique_id():
		# Maybe show a message to the user
		print("Control request denied for entity: ", entity_path)

func _on_visibility_changed():
	# Update entity list when the UI becomes visible
	if visible:
		var parent = get_parent()
		if parent and parent.get_parent() and parent.get_parent().has_method("get_entities"):
			call_deferred("update_entities", parent.get_parent().get_entities())
