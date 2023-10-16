## This particular module would provide chat capabilitis:
## 1. Window for chat + command/shortkey like Ctrl+T to show chat. 
## 
extends Node

#------------------------------------------
signal new_message(message: Message)
#------------------------------------------
var messages: = [Message]

#------------------------------------------
# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

#------------------------------------------


#------------------------------------------


func send_message(text: String):
	deliver_message.rpc(text, Profile.username, Profile.wallet)

#------------------------------------------
@rpc("any_peer", "call_local")
func deliver_message(text: String, sender_name: String, sender_wallet):
	var message = Message.new()
	
	message.text = text
	message.sender_name = sender_name
	message.sender_wallet = sender_wallet
	message.time = Time.get_unix_time_from_system()
	
	new_message.emit(message)
	messages.append(message)
	
#------------------------------------------
## Class to store messages.
class Message:
	var sender_name: =""
	var sender_wallet: = ""
	var time: float 
	var text: = ""
