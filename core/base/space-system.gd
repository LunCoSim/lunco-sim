# This is the basic class according to XTCE
class_name LCSpaceSystem
extends Node3D #TODO: Check if should be node, maybe something else?

#Basic parameters inspired by XTCE
@export var Telemetry = {}
@export var Parameters = {}
@export var Commands = {}

#----------------
@export var State = {} # Hierarchical state machine
@export var Behaviour = {} # Behaviour tree

#----------------
func process_command(command) -> bool:
	return true
