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
