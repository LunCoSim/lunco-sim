extends BaseResource

func _init():
	super._init()
	set_resource_properties("H2O", "Water resource", "product")
	mass = 18.015  # kg/unit (1 kmol)
	volume = 0.018  # m³/unit at standard conditions (1 kmol) 
