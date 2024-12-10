extends BaseResource

func _init():
	super._init()
	set_resource_properties("O2", "Oxygen resource", "product")
	mass = 1.0  # kg/unit
	volume = 0.7  # m³/unit at standard pressure 
