extends VBoxContainer

var STUB = "res://views/stub-view.tscn"


func _enter_tree():
	Input.set_mouse_mode(Input.MOUSE_MODE_VISIBLE)
	
func _on_Spaceflight_pressed():
	get_tree().change_scene("res://views/space-flight.tscn")

func _on_ModelBrowser_pressed():
	get_tree().change_scene("res://views/models-preview.tscn")

func _on_Trajectory_Planning_pressed():
	get_tree().change_scene(STUB)

func _on_Surface_Operations_pressed():
	get_tree().change_scene("res://views/matrix-view.tscn")

func _on_Help_pressed():
	OS.shell_open("https://github.com/LunCoSim/lunco-sim")

func _on_Settings_pressed():
	get_tree().change_scene(STUB)

func _on_Exit_pressed():
	get_tree().quit() # default behavior
	
func _on_Return_To_Main_Menu_pressed():
	get_tree().change_scene("res://views/main-menu.tscn")
