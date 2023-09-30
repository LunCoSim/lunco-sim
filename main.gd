## This is a starting script
extends Node


func _init():
	# Hack for compatibility with IVoyager
	IVGlobal.settings.gui_size = 1

	
func _ready():
	if ("--server" in OS.get_cmdline_args()) or (OS.has_feature("server")):
		print("Running in server mode")
		# Run your server startup code here...
		# Using this check, you can start a dedicated server by running
		# a Godot binary (headless or not) with the `--server` command-line argument.
		_on_server()
	else:
		_on_server()
		
#---------------

func _on_server():
#	StateManager.Username = %Username.text
	
	print("[INFO] _on_sim_host_pressed")
	
	StateManager.change_scene("sim")
	
func _on_local():
	print("_on_sim_client_pressed")
	var ip = %IP.text
	var port = %Port.text.to_int()
	
	LCNet.connect_to_server(ip, port)
	
	StateManager.change_scene("sim")

func _on_global():
	#default global server
	LCNet.connect_to_server("langrenus.lunco.space", 9000) #TBD Change to constants/settings/config?
	
	StateManager.change_scene("sim")
