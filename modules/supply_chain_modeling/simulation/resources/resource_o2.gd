class_name ResourceO2
extends BaseResource

func _init(name: String):
	super._init(name)
	mass = 31.998  # g/mol
	volume = 0.0  # Placeholder value
	set_properties("Oxygen", "product", mass, volume)
