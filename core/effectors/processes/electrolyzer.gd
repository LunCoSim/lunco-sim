class_name LCElectrolyzer
extends LCProcessEffector

## Water Electrolyzer
##
## Splits water into hydrogen and oxygen using electrical power.

func _ready():
	super._ready()
	recipe_id = "water_electrolysis"
	_load_recipe()
