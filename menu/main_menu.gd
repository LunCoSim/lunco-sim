# Old code to be removed later
extends Node

# Called when the node enters the scene tree for the first time.
func _ready():
	print("Main ready")
	pass # Replace with function body.
	
	if ("--server" in OS.get_cmdline_args()) or (OS.has_feature("server")):
		print("Running in server mode")
		# Run your server startup code here...
		# Using this check, you can start a dedicated server by running
		# a Godot binary (headless or not) with the `--server` command-line argument.
		_on_sim_host_pressed()


# ------------------------------------


func change_scene(scene: String):
	SceneManager.no_effect_change_scene(scene)

## UI Integrations
func _on_sim_host_pressed():
	
	StateManager.Username = %Username.text
	
	print("[INFO] _on_sim_host_pressed")
	
#	net.host(9000)
	
	change_scene("sim")

func _on_sim_client_pressed():
	print("_on_sim_client_pressed")
	var ip = %IP.text
	var port = %Port.text.to_int()
	
	LCNet.connect_to_server(ip, port)
	
	change_scene("sim")
	
func _on_yarm_pressed():
	change_scene("yarm")

func _on_connect_to_global_pressed():
	#default global server
	LCNet.connect_to_server("langrenus.lunco.space", 9000)
	
	change_scene("sim")


func _on_whiteboard_pressed():
	change_scene("whiteboard")

func _on_text_editor_pressed():
	change_scene("editor")

func _on_future_missions_pressed():
	change_scene("FutureLunarMissions")
