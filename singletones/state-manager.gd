class_name lsStateManager
extends Node

@export var Username := ""

# Called when the node enters the scene tree for the first time.
func _ready():
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

func _save():
	pass
	
func _load():
	pass

#
func change_scene(scene: String):
	SceneManager.no_effect_change_scene(scene)
