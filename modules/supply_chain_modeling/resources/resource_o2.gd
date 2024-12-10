extends BaseResource

func _init():
	super._init()
	set_resource_properties("O2", "Oxygen resource", "product")
	mass = 32.0  # kg/unit (where unit = kmol)
	volume = 22.4  # m³/unit at standard temperature and pressure (STP)
