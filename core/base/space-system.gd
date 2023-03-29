# This is the basic class according to XTCE
extends Node3D #TODO: Check if should be node, maybe something else?
class_name lnSpaceSystem


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
