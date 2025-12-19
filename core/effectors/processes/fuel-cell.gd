class_name LCFuelCell
extends LCProcessEffector

## Fuel Cell
##
## Generates electrical power by combining hydrogen and oxygen.
## Produces water as a byproduct.

@export var power_output: float = 0.0  # Current power output in Watts

func _ready():
	super._ready()
	recipe_id = "fuel_cell"
	auto_start = true  # Auto-start when inputs available
	_load_recipe()

func _physics_process(delta: float):
	super._physics_process(delta)
	
	# Calculate actual power output
	if is_active and recipe:
		power_output = recipe.power_required * current_efficiency
	else:
		power_output = 0.0

func get_power_output() -> float:
	return power_output
