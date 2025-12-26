extends Node

## Manages the visibility and lifecycle of the Command UI.
## Attached as an Autoload: LCCommandUI

func _ready():
	print("[LCCommandUI] Manager ready.")
	process_mode = Node.PROCESS_MODE_ALWAYS

func _input(event):
	# Hardcoded fallback check for F10 (decimal 4194313 or KEY_F10)
	var is_f10 = event is InputEventKey and event.pressed and (event.keycode == KEY_F10 or event.physical_keycode == KEY_F10)
	
	if event.is_action_pressed("toggle_command_ui") or is_f10:
		print("[LCCommandUI] Toggle triggered. Event: ", event)
		toggle_ui()
		get_viewport().set_input_as_handled()

func toggle_ui():
	var lw = get_node_or_null("/root/LCWindows")
	if lw:
		print("[LCCommandUI] Found LCWindows. Toggling...")
		lw.toggle_command_ui()
	else:
		# Check if it exists under the name it was registered with in project.godot
		push_error("[LCCommandUI] CRITICAL: LCWindows autoload node not found in /root. Check for script errors in windows-manager.gd")
		# Log all children of root for debugging
		print("[LCCommandUI] Root children: ", get_node("/root").get_children())
