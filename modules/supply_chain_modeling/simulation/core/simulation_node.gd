class_name SimulationNode
extends Node

@export var description: String

# Default properties
var default_description: String = ""

# Function to save the state of the node
func save_state() -> Dictionary:
	return {
		"description": description
	}

func load_state(state: Dictionary) -> void:
	if state:
		description = state.get("description", default_description)
