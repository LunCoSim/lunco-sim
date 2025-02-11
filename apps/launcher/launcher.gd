extends Control

func _ready():
	$VBoxContainer/SupplyChainButton.pressed.connect(_on_supply_chain_pressed)
	$VBoxContainer/MainButton.pressed.connect(_on_main_pressed)

func _on_supply_chain_pressed():
	# Load supply chain scene
	get_tree().change_scene_to_file("res://apps/supply_chain_modeling/rsct.tscn")

func _on_main_pressed():
	# Check if running on web platform
	if OS.has_feature("web"):
		# Open 3D sim in new window if on web
		OS.shell_open("https://alpha.lunco.space/3dsim/index.html")
	else:
		# Load main scene if not on web
		get_tree().change_scene_to_file("res://apps/3dsim/main.tscn")

func _on_main_animated_pressed():
	# Load main animated scene
	get_tree().change_scene_to_file("res://main_animated.tscn") 
