extends Control

signal entity_selected(int)
signal existing_entity_selected(int)

@onready var ui := %TargetUI

var _ui
var avatar: LCAvatar
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.
	
	var tree: ItemList = %Entities
	
	for entity in EntitiesDB.Entities:
		# Add child items to the root.
		tree.add_item(str(entity))
	
	avatar = get_parent()
	
	tree.select(avatar.entity_to_spawn)
	
	Users.users_updated.connect(_on_update_connected_users)
	Profile.profile_changed.connect(_on_profile_changed)
	
	_update_connection_status()
	
	# Add this line to update the user list when the scene is ready
	_on_update_connected_users()

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass


# Function set_ui clears the ui and sets target if ui exists
func set_ui(_ui=null):
	clear_ui()
	if(_ui):
		ui.add_child(_ui)
		

# Function clear_ui removes child items if ui exists	
func clear_ui():
	if ui:
		for n in ui.get_children():
			ui.remove_child(n)

func set_target(target):
	
	if target is LCCharacterController:
		_ui = load("res://controllers/character/character-ui.tscn").instantiate()
	elif target is LCSpacecraftController:
		_ui = load("res://controllers/spacecraft/spacecraft-ui.tscn").instantiate()
	elif target is LCOperatorController:
		_ui = load("res://controllers/operator/operator-ui.tscn").instantiate()

	if _ui:
		_ui.set_target(target) #controller specific function
	set_ui(_ui)
	
	update_entities(get_parent().get_parent().entities) #TBD Very dirty hack! Getting Universe entities
	
func _on_entities_item_selected(index):
	print("_on_entities_item_selected: ", index)
	emit_signal("entity_selected", index)
	pass # Replace with function body.

func _on_existing_entity_selected(idx):
	existing_entity_selected.emit(idx)
	
func update_entities(entities):
	
	var tree: HBoxContainer = %LiveEntities
	
	for child in tree.get_children():
		child.queue_free()
	
	var idx = 0
	for entity in entities:
		# Add child items to the root.
		var button = Button.new()
		var entity_name = str(entity.name)
		
		# Check if the entity has a multiplayer authority
		# if entity.get("multiplayer_authority"):
		var owner_id = entity.get_multiplayer_authority()
		entity_name += " (Owner: " + str(owner_id) + ")"
		
		button.text = entity_name
		tree.add_child(button)

		button.pressed.connect(_on_existing_entity_selected.bind(idx))
		button.flat = true
		if avatar.target and entity == avatar.target.get_parent():
			button.flat = false
		idx += 1

func _on_update_connected_users():
	var tree: ItemList = %Users
	
	tree.clear()
	for user_id in Users.users:
		var username = Users.users[user_id]["username"]
		tree.add_item(username)

func select_entity(idx):
	var tree: HBoxContainer = %LiveEntities
	
	for child: Button in tree.get_children():
		child.flat = false
		if child.get_index() == idx:
			child.flat = true

func _on_profile_changed():
	_update_connection_status()

func _update_connection_status():
	%ConnectWallet.visible = Profile.wallet == ""
	%DisconnectWallet.visible = Profile.wallet != ""

	

	%WalletInfoLabel.text = ("Connected: " + Profile.wallet.substr(0, 8) + "...") if Profile.wallet != "" else "Not connected"
	%ProfileNFT.text = "Yes" if Profile.has_profile > 0 else "No"
	%GitcoinDonor.text = "Yes" if Profile.is_donor() else "No"
	%ArtizenBuyer.text = "Yes" if Profile.is_artizen_buyer else "No"

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
