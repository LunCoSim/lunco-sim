class_name EntitiesDB
extends Node

enum Entities {
	Spacecraft,
	Operator,
	Gobot,
	Astronaut
}

var Paths = {
	Entities.Spacecraft: "res://entities/starship/starship.tscn",
	Entities.Operator: "res://entities/operator/operator.tscn",
	Entities.Gobot: "res://entities/gobot/gobot.tscn",
	Entities.Astronaut: "res://entities/astronaut/astronaut.tscn",
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

