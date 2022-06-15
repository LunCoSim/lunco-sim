# This is basically the simulation world
# Check terms for more information
class_name lnMatrix
extends lnSpaceSystem

#TODO: Introduce GravitaionField class that describes gravitation field in 4D space e.g. gravitation = F(time, x, y, z)
var GravitationField := lnGravitationField.new()
var LocalTime

onready var player = $Player

# Other members:
# World
# 	Celestial bodies
# 		Sun
#			Planets
# 				Moon
# 				Earth
# 				Mars
# 		Others
# SpaceSystems aka objects:
# 	Players
#	Cameras
#	Inputs
#	Vehicles
#	Robots
#	Buildings
#	Rovers
#	Rockets
#	Satellites
#	Space Stations
#	!!!Terrain 
# Avatars -> dynamic property, makes sense only in runtime 
# Fields 

# Objects
# Buildings
# Fields: gravitation, thermal, thermal flux, atmospheric resistance, e.t.c.
# Everything interacts with fields, fields interact with each other
# CFD is a field. 
# Fields can be local, meaning it's value is zero everywhere except finite space
# Fields can generate forces

# Avatars can connect to matrix
# Matrix can respond on requests, e.g. provide avater with all the objects 

# Common fields:
# - Gravitation field
# - Atmospheric field
# - Thermal field
# - Fossils field
# - Electric network/current field
# - Magnetic field
# - Radiation field

# Fields can be integrated and differinciated 

func ray_cast(from, to):
	var space_state  = $_World.get_world().direct_space_state
	
	return space_state.intersect_ray(from, to)
	
func create_object():
	pass

func get_players():
	pass
	
func get_rockets():
	pass
	
func get_operators():
	pass

#------------------------------------------
func spawn(position):
	print("Map clicked: ", position)
	var scene = load("res://addons/lunco-content/spacex-starship/source/SpaceX_Starship.fbx")
	var instance = scene.instance()
	
	instance.translation = position
	add_child(instance)


func _process(_delta):
	
	# TODO: this check must be done using timer e.g. check at rate of 10 hz/sec
	# Fade out to black if falling out of the map. -17 is lower than
	# the lowest valid position on the map (which is a bit under -16).
	# At 15 units below -17 (so -32), the screen turns fully black.
		
	if player:
		if player.transform.origin.y < -17:
	#		color_rect.modulate.a = min((-17 - player.transform.origin.y) / 15, 1)
			# If we're below -40, respawn (teleport to the initial position).
			if player.transform.origin.y < -40:
	#			color_rect.modulate.a = 0
				player.transform.origin = player.initial_position
