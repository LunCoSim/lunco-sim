class_name LCInputAdapter
extends Node

## Base class for all input adapters in the LunCoSim system.
## Provides common functionality for resolving targets through LCAvatar proxies.

@export var target: Node

## Resolves the actual target, handling LCAvatar indirection.
## If the target is an LCAvatar, returns the avatar's target.
## Otherwise, returns the target directly.
func get_resolved_target() -> Node:
	if target is LCAvatar:
		return target.target
	return target

## Checks if input should be processed based on global UI state.
## Returns false if a UI element (like Modelica display) has captured input.
func should_process_input() -> bool:
	# Check for UiDisplayManager and ask if input is captured
	var managers = get_tree().get_nodes_in_group("ui_display_manager")
	if managers.size() > 0:
		if managers[0].has_method("is_input_captured"):
			if managers[0].is_input_captured():
				return false
	
	return true
