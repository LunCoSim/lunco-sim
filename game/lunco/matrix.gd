# This is basically the simulation world
# Check terms for more information
class_name lnMatrix
extends Node

#TODO: Introduce GravitaionField class that describes gravitation field in 4D space e.g. gravitation = F(time, x, y, z)
var GravitationField := lnGravitationField.new()
var LocalTime

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

