extends UIBaseResource

func _init():
	super._init()
	if not resource:
		resource = BaseResource.new("H2")
	resource.mass = 2.016  # g/mol
	resource.volume = 0.0  # Placeholder value
	resource.set_properties("Hydrogen", "product", resource.mass, resource.volume)
