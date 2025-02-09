extends Control

func _ready():
	$VBoxContainer/SupplyChainButton.pressed.connect(_on_supply_chain_pressed)
	$VBoxContainer/MainButton.pressed.connect(_on_main_pressed)

func _on_supply_chain_pressed():
	# Load supply chain scene
	get_tree().change_scene_to_file("res://modules/supply_chain_modeling/rsct.tscn")

func _on_main_pressed():
	# Load main scene
	get_tree().change_scene_to_file("res://main.tscn")

func _on_main_animated_pressed():
	# Load main animated scene
	get_tree().change_scene_to_file("res://main_animated.tscn") 
