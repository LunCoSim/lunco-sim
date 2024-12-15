class_name SimulationNode
extends Node

var node_id: String
var node_type: String
var properties: Dictionary = {}
var connections: Array = []

func _init(id: String, type: String):
	node_id = id
	node_type = type

func to_dict() -> Dictionary:
	return {
		"id": node_id,
		"type": node_type,
		"properties": properties,
		"connections": connections
	}
