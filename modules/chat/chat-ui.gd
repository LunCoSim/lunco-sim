extends VBoxContainer

# Called when the node enters the scene tree for the first time.
func _ready():
	Chat.new_message.connect(_on_new_message)


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

func _on_new_message(new_message, sender_name, sender_wallet):
	var text = sender_name + ": " + new_message.text
	
	var msg: ItemList = %Messages
	
	msg.add_item(text)
	
	
func _on_send_button_pressed():
	Chat.send_message(%TextEdit.text)
	%TextEdit.text = ""
