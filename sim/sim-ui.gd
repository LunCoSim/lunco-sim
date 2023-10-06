extends CanvasLayer


# Called when the node enters the scene tree for the first time.
func _ready():
#	set_mouse_filter(Control.MOUSE_FILTER_PASS)
	pass

# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta):
	pass

func target_changed(target):
	if target is LCCharacter:
		$Help/Target.text = "Target: Player"
	elif target is LCSpacecraft:
		$Help/Target.text = "Target: Spacecraft"
	elif target is LCOperator:
		$Help/Target.text = "Target: Operator"
	else:
		$Help/Target.text = "Target: Unknown"
