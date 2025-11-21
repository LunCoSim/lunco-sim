extends Control

@onready var target: Node3D

func set_target(_target):
	target = _target



func _on_HideControls_timeout():
	$Help.visible = false
