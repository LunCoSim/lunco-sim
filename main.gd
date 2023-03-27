extends Node


# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass


func _on_sim_pressed():
	get_tree().change_scene_to_file("res://apps/sim/app.tscn")


func _on_yarm_pressed():
	get_tree().change_scene_to_file("res://apps/yarm/app.tscn")
