class_name LCEntitiesManager
extends Node

enum Entities {
	Spacecraft,
	Operator,
	Gobot,
	Astronaut,
	Rover,
	LunarLander,
	MarsRover,
	RoverRigid,
}

var Paths = {
	Entities.Spacecraft: "res://content/starship/starship.tscn",
	Entities.Operator: "res://core/entities/operator.tscn",
	Entities.Gobot: "res://content/gobot/gobot.tscn",
	Entities.Astronaut: "res://content/animated-astronaut-character-in-space-suit-loop/astronaut.tscn",
	Entities.Rover: "res://apps/3dsim/entities/rover/rover.tscn",
	Entities.LunarLander: "res://core/entities/vehicles/lunar_lander.tscn",
	Entities.MarsRover: "res://core/entities/vehicles/mars_rover.tscn",
	Entities.RoverRigid: "res://apps/3dsim/entities/rover_rigid.tscn",
}

var UIs = {
	Entities.Spacecraft: "res://content/starship/starship-ui.tscn",
	Entities.Operator: "res://entities/operator/operator.tscn",
	Entities.Gobot: "res://entities/gobot/gobot.tscn",
	Entities.Astronaut: "res://entities/astronaut/astronaut.tscn",
	Entities.Rover: "res://apps/3dsim/entities/rover/rover.tscn",
	Entities.LunarLander: "res://content/starship/starship-ui.tscn", # Reuse starship UI for now
	Entities.MarsRover: "res://apps/3dsim/entities/rover/rover.tscn", # Reuse rover UI
	Entities.RoverRigid: "res://apps/3dsim/entities/rover_rigid.tscn",
}

var InputAdapters = {
	Entities.Spacecraft: "res://content/starship/starship.tscn",
	Entities.Operator: "res://entities/operator/operator.tscn",
	Entities.Gobot: "res://entities/gobot/gobot.tscn",
	Entities.Astronaut: "res://entities/astronaut/astronaut.tscn",
	Entities.Rover: "res://apps/3dsim/entities/rover/rover.tscn",
	Entities.LunarLander: "res://content/starship/starship.tscn", # Reuse starship adapter
	Entities.MarsRover: "res://apps/3dsim/entities/rover/rover.tscn", # Reuse rover adapter
	Entities.RoverRigid: "res://apps/3dsim/entities/rover_rigid.tscn",
}

var Caches: = {
	
}

func _init():
	for entity in Paths:
		var path : String = Paths[entity]
		ResourceLoader.load_threaded_request(path)
		Caches[entity] = load(path)
		
func make_entity(entity):
	if not Paths.has(entity):
		push_error("EntitiesManager: Unknown entity: " + str(entity))
		return null
		
	var path : String = Paths[entity]
	
	if Caches.get(entity) != null and Caches[entity] is PackedScene:
		return Caches[entity].instantiate()
	else:
		var scene = ResourceLoader.load_threaded_get(path)
		if scene == null:
			# Fallback to sync load if threaded get fails (e.g. not ready)
			scene = load(path)
			
		if scene != null:
			Caches[entity] = scene
			return scene.instantiate()
		else:
			push_error("EntitiesManager: Failed to load scene for entity: " + str(entity) + " at path: " + path)
			return null
##
