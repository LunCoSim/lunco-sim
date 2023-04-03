extends Node

var peer = ENetMultiplayerPeer.new()

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.
	
	if ("--server" in OS.get_cmdline_args()) or (OS.has_feature("server")):
		# Run your server startup code here...
		# Using this check, you can start a dedicated server by running
		# a Godot binary (headless or not) with the `--server` command-line argument.
		_on_sim_host_pressed()


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

## UI Integrations
func _on_sim_host_pressed():
	peer.create_server(9000)
	multiplayer.multiplayer_peer = peer
	
	get_tree().change_scene_to_file("res://apps/sim/app.tscn")

func _on_sim_client_pressed():
	var ip = %IP.text
	var port = %Port.text.to_int()
	
	peer.create_client(ip, port)
	multiplayer.multiplayer_peer = peer
	
	get_tree().change_scene_to_file("res://apps/sim/app.tscn")
	pass # Replace with function body.
	
func _on_yarm_pressed():
	get_tree().change_scene_to_file("res://apps/yarm/app.tscn")
