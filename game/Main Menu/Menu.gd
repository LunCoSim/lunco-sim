extends VBoxContainer


# Declare member variables here. Examples:
# var a = 2
# var b = "text"


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
#func _process(delta):
#	pass

# Sets the scene to the spaceflight scene
func _on_Spaceflight_pressed():
	get_tree().change_scene("res://Spaceflight.tscn")
	pass # Replace with function body.


func _on_Trajectory_Planning_pressed():
	get_tree().change_scene("res://temp.tscn")
	pass # Replace with function body.


func _on_Surface_Operations_pressed():
	get_tree().change_scene("res://temp.tscn")
	pass # Replace with function body.


func _on_Lunar_Base_Sim_pressed():
	get_tree().change_scene("res://temp.tscn")
	pass # Replace with function body.


func _on_Help_pressed():
	OS.shell_open("https://github.com/LunCoSim/lunco-sim")
	pass # Replace with function body.


func _on_Settings_pressed():
	get_tree().change_scene("res://temp.tscn")
	pass # Replace with function body.


func _on_Exit_pressed():
	get_tree().quit() # default behavior
	pass # Replace with function body.
	

func _on_Return_To_Main_Menu_pressed():
	get_tree().change_scene("res://Main Menu/Main Menu.tscn")
	pass # Replace with function body.
