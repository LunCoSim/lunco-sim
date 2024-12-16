extends UIBaseResource

func _init():
	super._init()
	if not resource:
		resource = BaseResource.new("O2")
	resource.mass = 31.998  # g/mol
	resource.volume = 0.0  # Placeholder value
	resource.set_properties("Oxygen", "product", resource.mass, resource.volume)
