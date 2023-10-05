class_name EntitiesDB
extends Node

enum Entities {
	Spacecraft,
	Operator,
	Character,
	Astronaut
}

var Paths = {
	Entities.Spacecraft: "res://entities/starship-entity.tscn",
	Entities.Operator: "res://entities/operator-entity.tscn",
	Entities.Character: "res://entities/character-entity.tscn",
	Entities.Astronaut: "res://entities/astronaut-entity.tscn",
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

