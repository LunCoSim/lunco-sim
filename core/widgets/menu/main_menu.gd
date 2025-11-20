# Old code to be removed later
extends Node


#------------------------------------

func _ready():
	# Set the version from project settings
	$MarginContainer/ScrollContainer/MainContent/Version.text = "v" + str(ProjectSettings.get_setting("application/config/version"))
	on_reload_profile()
	
	Profile.profile_changed.connect(on_reload_profile)

func on_reload_profile():
	%Username.text = Profile.username
	%Wallet.text = Profile.wallet
#------------------------------------

func _on_back_to_launcher_pressed():
	get_tree().change_scene_to_file("res://launcher/launcher.tscn")
	LCWindows.toggle_main_menu()

func change_scene(scene: String):
	pass
	#SceneManager.no_effect_change_scene(scene)

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

var _on_wallet_connected_callback = JavaScriptBridge.create_callback(on_wallet_connected) # This reference must be kept
var _on_check_profile_nft_callback = JavaScriptBridge.create_callback(on_check_profile_nft) # This reference must be kept
	
func _on_connect_wallet_pressed():
	JavaScriptBridge.get_interface("Login").login(_on_wallet_connected_callback)

func on_wallet_connected(args):
	print("on_wallet_connected: ")
	print(args[0])
	print(args[0]["wallet"])
	Profile.wallet = args[0]["wallet"]

func on_check_profile_nft(args):
	print("on_account_profile: ")
	print(args[0])
	Profile.has_profile = int(args[0])
	
	if Profile.has_profile > 0:
		$MarginContainer/ScrollContainer/MainContent/ProfilePanel/Profile/ProfileButtonsContainer/CheckProfileNFT.text = "You own " + str(Profile.has_profile) + " Profile NFT"
	else:
		$MarginContainer/ScrollContainer/MainContent/ProfilePanel/Profile/ProfileButtonsContainer/CheckProfileNFT.text = "No Profile NFT. Go get one!"
	
func _on_check_profile_nft_pressed():
	print("_on_check_profile_nft_pressed: ", Profile.wallet)
	JavaScriptBridge.get_interface("Login").checkProfile(Profile.wallet, _on_check_profile_nft_callback)

func _on_replay_mode_pressed():
	LCWindows.toggle_main_menu() # Close the menu
	StateManager.goto_replay_scene()
