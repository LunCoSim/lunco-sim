# Old code to be removed later
extends Node


#------------------------------------

func _ready():
	$Version.text = "v. " + str(ProjectSettings.get_setting("application/config/version"))
	
#------------------------------------

func change_scene(scene: String):
	SceneManager.no_effect_change_scene(scene)

## UI Integrations
func _on_sim_host_pressed():
	
	StateManager.Username = %Username.text
	
	print("[INFO] _on_sim_host_pressed")
	
	LCNet.host(9000)
	
	#change_scene("sim")

func _on_sim_client_pressed():
	print("_on_sim_client_pressed")
	var ip = %IP.text
	var port = %Port.text.to_int()
	
	LCNet.connect_to_server(ip, port)
	
	#change_scene("sim")

func _on_connect_to_global_pressed():
	#default global server
	LCNet.connect_to_server()
	
	#change_scene("sim")


