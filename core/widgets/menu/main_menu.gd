# Old code to be removed later
extends Node


#------------------------------------

func _ready():
	$Version.text = "v. " + str(ProjectSettings.get_setting("application/config/version"))
	on_reload_profile()
	
	Messenger.profile_wallet_changed.connect(on_reload_profile)

func on_reload_profile():
	%Username.text = Profile.username
	%Wallet.text = Profile.wallet
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




func _on_username_text_changed(new_text):
	Profile.username = new_text

var _my_js_callback = JavaScriptBridge.create_callback(on_wallet_connected) # This reference must be kept

	
func _on_connect_wallet_pressed():
	JavaScriptBridge.get_interface("Login").login(_my_js_callback)

func on_wallet_connected(args):
	print("on_wallet_connected: ")
	print(args[0])
	print(args[0]["wallet"])
	Profile.wallet = args[0]["wallet"]
	
	
func _on_check_profile_nft_pressed():
	pass # Replace with function body.
