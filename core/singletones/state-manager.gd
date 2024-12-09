class_name lsStateManager
extends Node

@export var Username := ""

func _ready() -> void:
	Panku.hide()
	
func _save():
	pass
	
func _load():
	pass

func change_scene(scene: String):
	SceneManager.no_effect_change_scene(scene)
