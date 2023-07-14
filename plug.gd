## List of used addons for gd-plug
## After adding addon run install_addons.sh
extends "res://addons/gd-plug/plug.gd"

func _plugging():
	#UI
	plug("folt-a/godot-idea-board")
	plug("imjp94/gd-plug-ui")
	plug("imjp94/UIDesignTool")
	

	#Importers
	plug("timothyqiu/godot-csv-data-importer")
	plug("elenakrittik/GodotXML")

	#Nodes & Behaviour
	# plug("bitbrain/beehave") ## somehow it's not working properly, and as it's not used now - commented
	plug("imjp94/gd-YAFSM")
	
	#Developer Tools
	plug("Ark2000/PankuConsole")
	plug("godot-extended-libraries/godot-debug-menu")

	## Libraries
	# plug("maktoobgar/scene_manager")
	plug("maktoobgar/scene_manager", {"on_updated"="on_scene_manager_updated"}) ## Scene manager

func on_scene_manager_updated(plugin):
	print("Scene manager on updated. Must be done manually for now.")
	print("1. Remove file res://addons/scene_manager/scenes.gd")
	print("2. Update path to res://data/scene-manager/scenes.gd in files:")
	print("  -res://addons/scene_manager/manager.gd")
	print("  -res://addons/scene_manager/plugin.gd")
	print("Corresponging issue on github: https://github.com/maktoobgar/scene_manager/issues/9")
