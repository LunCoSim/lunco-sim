extends Control

var peer = ENetMultiplayerPeer.new()

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

func _on_InfiniteCanvas_mouse_entered():
	$InfiniteCanvas.enable()

# -------------------------------------------------------------------------------------------------
func _on_InfiniteCanvas_mouse_exited():
	$InfiniteCanvas.disable()


func _on_button_pressed():
	peer.create_client("127.0.0.1", 9000)
	multiplayer.multiplayer_peer = peer


func _on_button_2_pressed():
	peer.create_server(9000)
	multiplayer.multiplayer_peer = peer

func set_text(text):
	$TextEdit.text = text

func _on_text_edit_text_changed():
	
	Entities.set_text.rpc_id(1, $TextEdit.text)

	pass # Replace with function body.
