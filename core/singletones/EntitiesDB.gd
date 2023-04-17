class_name EntitiesDB
extends Node

enum Entities {
	Spacecraft,
	Operator,
	Character
}

var Paths = {
	Entities.Spacecraft: "res://core/entities/starship-entity.tscn",
	Entities.Operator: "res://core/entities/operator-entity.tscn",
	Entities.Character: "res://core/entities/player-entity.tscn"
}

var Caches = {
	
}

func _init():
	for entity in Paths:
		var path : String = Paths[entity]
		
		Caches[entity] = load(path)
		
func make_entity(entity):
	return Caches[entity].instantiate()
#
#var Caches = {
#	Entities.Spacecraft: preload,
#	Entities.Operator: "res://core/entities/player-entity.tscn",
#	Entities.Character: "res://core/entities/player-entity.tscn"
#}
#var PlayerEntity := preload()
#var OperatorEntity := preload("res://core/entities/operator-entity.tscn")
#var SpacecraftEntity := preload("res://core/entities/starship-entity.tscn")

@export var text := ""

@rpc("reliable")
func set_text(_text):
	print("is_server: ", multiplayer.is_server(), " set text: ", _text)
	text=_text
