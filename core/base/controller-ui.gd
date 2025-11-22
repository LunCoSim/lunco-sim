class_name LCControllerUI
extends Control

## Base class for all controller UI components in the LunCoSim system.
## Provides common functionality for setting and managing controller targets.

var target

## Sets the target controller for this UI component.
## Subclasses can override _on_target_set() to perform additional setup.
func set_target(_target):
	target = _target
	_on_target_set()

## Called after target is set. Override in subclasses for specific behavior.
## Example: connecting to controller signals, initializing UI elements, etc.
func _on_target_set():
	pass
