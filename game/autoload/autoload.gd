extends Node

func _ready():
	print("aslkdjaksldj")	
	
func _input(event):
	if event.is_action_pressed("main_menu"):
		goto_main()

func goto_main():
	get_tree().change_scene("res://main-menu/main-menu.tscn")
