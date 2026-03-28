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
	Entities.Spacecraft: "res://controllers/spacecraft/spacecraft-ui.tscn",
	Entities.Operator: "res://controllers/operator/operator-ui.tscn",
	Entities.Gobot: "res://controllers/character/character-ui.tscn",
	Entities.Astronaut: "res://controllers/character/character-ui.tscn",
	Entities.Rover: "res://controllers/rover/rover-ui.tscn",
	Entities.LunarLander: "res://controllers/spacecraft/spacecraft-ui.tscn",
	Entities.MarsRover: "res://controllers/rover/rover-ui.tscn",
	Entities.RoverRigid: "res://controllers/rover/rover-ui.tscn",
}

var InputAdapters = {
	Entities.Spacecraft: "res://controllers/spacecraft/spacecraft-input-adapter.tscn",
	Entities.Operator: "res://controllers/operator/operator-input-adapter.tscn",
	Entities.Gobot: "res://controllers/character/character-input-adapter.tscn",
	Entities.Astronaut: "res://controllers/character/character-input-adapter.tscn",
	Entities.Rover: "res://controllers/rover/rover-input-adapter.tscn",
	Entities.LunarLander: "res://controllers/spacecraft/spacecraft-input-adapter.tscn",
	Entities.MarsRover: "res://controllers/rover/rover-input-adapter.tscn",
	Entities.RoverRigid: "res://controllers/rover/rover_rigid_input_adapter.gd", # Note: Rigid uses script directly or needs a tscn
}

var Caches: = {
	
}

func _init():
	for entity in Paths:
		var path : String = Paths[entity]
		ResourceLoader.load_threaded_request(path)
		Caches[entity] = load(path)
		
func make_entity(entity):
	print("EntitiesManager: make_entity called for: ", entity)
	if not Paths.has(entity):
		push_error("EntitiesManager: Unknown entity: " + str(entity))
		return null
		
	var path : String = Paths[entity]
	print("EntitiesManager: Path for entity: ", path)
	
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
