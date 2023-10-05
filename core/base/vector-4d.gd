#represents point in continuum according to general relativity

class_name LCVector4D
extends Node

@export var time: int = 0 # J2000 e.g. seconds since Jan 1, 2000. Can be negative
@export var space: Vector3 = Vector3.ZERO
