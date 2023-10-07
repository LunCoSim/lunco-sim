# This is the basic class according to XTCE
@icon("res://core/base/space-system.svg")
class_name LCSpaceSystem
extends Node3D #TODO: Check if should be node, maybe something else?

@export var Visual: PackedScene
#Basic parameters inspired by XTCE
@export_category("XTCE")
@export var Telemetry = {}
@export var Parameters = {}
@export var Commands = {}

#----------------
@export_category("Behaviour")
@export var State = {} # Hierarchical state machine
@export var Behaviour = {} # Behaviour tree

#----------------
func process_command(_command) -> bool:
	return true
