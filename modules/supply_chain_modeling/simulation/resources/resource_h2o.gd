class_name ResourceH2O
extends BaseResource

func _init(name: String):
	super._init(name)
	mass = 18.015  # g/mol
	volume = 0.0  # Placeholder value
	set_properties("Water", "product", mass, volume)
