class_name ResourceH2
extends BaseResource

func _init(name: String):
	super._init(name)
	mass = 2.016  # g/mol
	volume = 0.0  # Placeholder value
	set_properties("Hydrogen", "product", mass, volume)
