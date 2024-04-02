## List of used addons for gd-plug
## After adding addon run ./install_addons.sh

extends "res://addons/gd-plug/plug.gd" ## it's the only addon to be added to git, the rest are managed by it

func _plugging():
	#UI
	# plug("folt-a/godot-idea-board")
	# plug("imjp94/gd-plug-ui")
	plug("imjp94/UIDesignTool")
	

	#Importers
	# plug("timothyqiu/godot-csv-data-importer") # TBD Update to IVoyager tsv importer
	# plug("elenakrittik/GodotXML")

	#Nodes & Behaviour
	# plug("bitbrain/beehave") ## somehow it's not working properly, and as it's not used now - commented
	plug("imjp94/gd-YAFSM")
	
	#Developer Tools
	# plug("Ark2000/PankuConsole")
	plug("LunCoSim/PankuConsole") #using LunCo's fork
	# plug("godot-extended-libraries/godot-debug-menu")
	plug("finepointcgi/Godot-Open-AI-GPT-Integration")
	plug("Ericdowney/SignalVisualizer")
	
	## Libraries
	plug("maktoobgar/scene_manager") ## Scene manager

	#manualy delete _premade directory TBD: Propos to change structure of the file as hard to update
	#TBD: Start using when ported to 4.2
	#plug("PunchablePlushie/godot-game-settings", {"install_root": "addons/ggs", "include": ["."]}) ## Game settings

	# plug("LunCoSim/godot-game-settings") ## Game settings

	## IVoyager integration
	# print("ivoyager_assets should be downloaded manually https://github.com/ivoyager/ivoyager/releases")
	# plug("ivoyager/ivoyager_table_importer", {"install_root": "addons/ivoyager_table_importer", "include": ["."]})
	# plug("ivoyager/ivoyager", {"install_root": "ivoyager", "include": ["."]})
	# plug("ivoyager/planetarium", {"install_root": "addons/ivoyager_planetarium", "include": ["si_base_units.gd"]})

	## Plugin to render Starts, TBD integration
	#https://gitlab.com/godotuniverse/starfield

	plug("deep-entertainment/godottpd")
	plug("KOBUGE-Games/godot-logger", {"install_root": "addons/godot-logger", "include": ["."]})


	#WebRTC GDExtension must be installed separatly and manually for native builds
	#Official extension https://github.com/godotengine/webrtc-native
