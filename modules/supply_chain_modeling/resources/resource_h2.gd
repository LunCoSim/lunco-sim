extends BaseResource

func _init():
	super._init()
	set_resource_properties("H2", "Hydrogen resource", "product")
	mass = 0.0899  # kg/unit (1 kmol)
	volume = 22.4  # m³/unit at standard conditions (1 kmol) 
