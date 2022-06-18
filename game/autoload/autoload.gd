extends Node

var lnSpaceSystem = load("res://lunco-core/space-system.gd")


func _ready():
	print("Autoload ready")	
	
func _input(event):
	if event.is_action_pressed("main_menu"):
		goto_main()

func goto_main():
	get_tree().change_scene("res://views/main-menu.tscn")
