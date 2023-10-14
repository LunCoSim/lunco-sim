@icon("res://controllers/player/character.svg")
## This controller represents character state and attaches to CharacterBody3D
class_name LCCharacterController
extends LCController

@export var character_body: CharacterBody3D

func _ready():
	if character_body == null:
		character_body = get_parent()
