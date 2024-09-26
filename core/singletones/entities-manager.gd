class_name LCEntitiesManager
extends Node

enum Entities {
	Spacecraft,
	Operator,
	Gobot,
	Astronaut,
}

var Paths = {
	Entities.Spacecraft: "res://content/starship/starship.tscn",
	Entities.Operator: "res://core/entities/operator.tscn",
	Entities.Gobot: "res://content/gobot/gobot.tscn",
	Entities.Astronaut: "res://content/animated-astronaut-character-in-space-suit-loop/astronaut.tscn"
}

var UIs = {
	Entities.Spacecraft: "res://content/starship/starship-ui.tscn",
	Entities.Operator: "res://entities/operator/operator.tscn",
	Entities.Gobot: "res://entities/gobot/gobot.tscn",
	Entities.Astronaut: "res://entities/astronaut/astronaut.tscn",
}

var InputAdapters = {
	Entities.Spacecraft: "res://content/starship/starship.tscn",
	Entities.Operator: "res://entities/operator/operator.tscn",
	Entities.Gobot: "res://entities/gobot/gobot.tscn",
	Entities.Astronaut: "res://entities/astronaut/astronaut.tscn",
}


var Caches: = {
	
}

func _init():
	for entity in Paths:
		var path : String = Paths[entity]
		ResourceLoader.load_threaded_request(path)
		Caches[entity] = load(path)
		
func make_entity(entity):
	var path : String = Paths[entity]
	#return .instantiate()
	#
	if Caches.get(entity) != null:
		return Caches[entity].instantiate()
	else:
		Caches[entity] = ResourceLoader.load_threaded_get(path)
		return Caches[entity].instantiate()
##
