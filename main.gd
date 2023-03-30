extends Node

var peer = ENetMultiplayerPeer.new()

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

## UI Integrations
func _on_sim_host_pressed():
	peer.create_server(9000)
	multiplayer.multiplayer_peer = peer
	
	get_tree().change_scene_to_file("res://apps/sim/app.tscn")

func _on_sim_client_pressed():
	peer.create_client("localhost", 9000)
	multiplayer.multiplayer_peer = peer
	
	get_tree().change_scene_to_file("res://apps/sim/app.tscn")
	pass # Replace with function body.
	
func _on_yarm_pressed():
	get_tree().change_scene_to_file("res://apps/yarm/app.tscn")
