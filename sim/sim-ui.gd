extends CanvasLayer


# Called when the node enters the scene tree for the first time.
func _ready():
#	set_mouse_filter(Control.MOUSE_FILTER_PASS)
	pass

func target_changed(target):
	if target is LCCharacterController:
		$Help/Target.text = "Target: Player"
	elif target is LCSpacecraftController:
		$Help/Target.text = "Target: Spacecraft"
	elif target is LCOperatorController:
		$Help/Target.text = "Target: Operator"
	else:
		$Help/Target.text = "Target: Unknown"
