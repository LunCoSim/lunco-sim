## This particular module would provide chat capabilitis:
## 1. Window for chat + command/shortkey like Ctrl+T to show chat. 
## 
extends Node

#------------------------------------------
signal new_message(message: Message)
signal private_message(message: Message)
signal system_message(message: Message)
signal profile_wallet_changed  # Moved from messenger
#------------------------------------------

enum MessageType {
	CHAT,
	SYSTEM,
	PRIVATE,
	COMMAND
}

## Class to store messages.
class Message:
	var sender_name: String = ""
	var sender_wallet: String = ""
	var time: float
	var text: String = ""
	var type: MessageType = MessageType.CHAT
	var recipients: Array[int] = []  # For private messages
	
	func is_private() -> bool:
		return type == MessageType.PRIVATE
		
	func is_system() -> bool:
		return type == MessageType.SYSTEM
		
	func is_command() -> bool:
		return type == MessageType.COMMAND

var messages: Array[Message] = []
var command_prefix := "/"

#------------------------------------------
# Called when the node enters the scene tree for the first time.
func _ready():
	# Connect to network events
	if multiplayer.is_server():
		multiplayer.peer_connected.connect(_on_peer_connected)
		multiplayer.peer_disconnected.connect(_on_peer_disconnected)

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

#------------------------------------------


#------------------------------------------


func send_message(text: String):
	if text.begins_with("/"):
		_handle_command(text)
	else:
		deliver_message.rpc(text, Profile.username, Profile.wallet, MessageType.CHAT)

func send_private_message(recipient_id: int, text: String):
	deliver_private_message.rpc_id(recipient_id, text, Profile.username, Profile.wallet)

func broadcast_system_message(text: String):
	if multiplayer.is_server():
		deliver_message.rpc(text, "SYSTEM", "", MessageType.SYSTEM)

#------------------------------------------
@rpc("any_peer", "call_local")
func deliver_message(text: String, sender_name: String, sender_wallet: String, type: MessageType = MessageType.CHAT):
	var message = Message.new()
	
	message.text = text
	message.sender_name = sender_name
	message.sender_wallet = sender_wallet
	message.time = Time.get_unix_time_from_system()
	message.type = type
	
	messages.append(message)
	new_message.emit(message)
	
	if type == MessageType.SYSTEM:
		system_message.emit(message)

@rpc("any_peer", "call_local")
func deliver_private_message(text: String, sender_name: String, sender_wallet: String):
	var message = Message.new()
	
	message.text = text
	message.sender_name = sender_name
	message.sender_wallet = sender_wallet
	message.time = Time.get_unix_time_from_system()
	message.type = MessageType.PRIVATE
	message.recipients = [multiplayer.get_unique_id()]
	
	messages.append(message)
	private_message.emit(message)

func _handle_command(text: String):
	var parts = text.split(" ")
	var command = parts[0].substr(1).to_lower()
	var args = parts.slice(1)
	
	match command:
		"whisper", "w", "msg":
			if args.size() >= 2:
				var recipient = args[0]
				var msg = " ".join(args.slice(1))
				# Find recipient ID from username
				var recipient_id = Users.find_user_id_by_name(recipient)
				if recipient_id != -1:
					send_private_message(recipient_id, msg)
				else:
					deliver_message.rpc_id(multiplayer.get_unique_id(), "User not found: " + recipient, "SYSTEM", "", MessageType.SYSTEM)
		"help":
			var help_text = """
			Available commands:
			/whisper (or /w, /msg) <username> <message> - Send private message
			/help - Show this help
			"""
			deliver_message.rpc_id(multiplayer.get_unique_id(), help_text, "SYSTEM", "", MessageType.SYSTEM)
		_:
			deliver_message.rpc_id(multiplayer.get_unique_id(), "Unknown command: " + command, "SYSTEM", "", MessageType.SYSTEM)

func _on_peer_connected(id: int):
	if multiplayer.is_server():
		broadcast_system_message("User connected: " + str(id))

func _on_peer_disconnected(id: int):
	if multiplayer.is_server():
		broadcast_system_message("User disconnected: " + str(id))

func get_recent_messages(count: int = 50) -> Array[Message]:
	return messages.slice(-count) if messages.size() > count else messages

func clear_messages():
	messages.clear()
