extends UIBaseResource

func _init():
	super._init()
	if not resource:
		resource = ResourceH2O.new()
