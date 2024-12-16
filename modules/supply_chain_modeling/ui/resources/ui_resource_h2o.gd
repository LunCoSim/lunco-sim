extends UIBaseResource

func _init():
	super._init()
	if not resource:
		resource = BaseResource.new("H2O")
	resource.mass = 18.015  # g/mol
	resource.volume = 0.0  # Placeholder value
	resource.set_properties("Water", "product", resource.mass, resource.volume)
