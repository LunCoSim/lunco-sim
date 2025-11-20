extends VBoxContainer

# Called when the node enters the scene tree for the first time.
func _ready():
	Chat.new_message.connect(_on_new_message)


func _on_new_message(message):
	var text = message.sender_name + ": " + message.text
	
	var msg: ItemList = %Messages
	
	msg.add_item(text)
	
	
func _on_send_button_pressed():
	Chat.send_message(%TextEdit.text)
	%TextEdit.text = ""


func _on_text_edit_text_submitted(new_text):
	_on_send_button_pressed()
