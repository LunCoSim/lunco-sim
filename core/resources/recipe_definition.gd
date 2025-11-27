class_name LCProcessRecipe
extends Resource

## Defines a resource conversion process
##
## Recipes specify inputs, outputs, duration, and energy requirements.
## Used by process effectors to convert resources.

@export var recipe_id: String = ""
@export var recipe_name: String = ""
@export_multiline var description: String = ""

@export_group("Process Parameters")
@export var duration: float = 1.0  ## Seconds per cycle
@export var power_required: float = 0.0  ## Watts
@export var heat_generated: float = 0.0  ## Watts of waste heat

@export_group("Inputs")
@export var input_resources: Array[LCProcessIngredient] = []

@export_group("Outputs")
@export var output_resources: Array[LCProcessProduct] = []

## Get efficiency (output value / input value)
func get_efficiency() -> float:
	# Simplified - could be more sophisticated
	var input_mass = 0.0
	var output_mass = 0.0
	
	for ingredient in input_resources:
		input_mass += ingredient.amount_per_cycle
	
	for product in output_resources:
		output_mass += product.amount_per_cycle
	
	if input_mass > 0:
		return output_mass / input_mass
	return 0.0

## Convert to dictionary (for JSON export)
func to_dict() -> Dictionary:
	var inputs = []
	for ing in input_resources:
		inputs.append(ing.to_dict())
	
	var outputs = []
	for prod in output_resources:
		outputs.append(prod.to_dict())
	
	return {
		"id": recipe_id,
		"name": recipe_name,
		"description": description,
		"duration": duration,
		"power_required": power_required,
		"heat_generated": heat_generated,
		"inputs": inputs,
		"outputs": outputs
	}

## Create from dictionary (for JSON import)
static func from_dict(data: Dictionary) -> LCProcessRecipe:
	var recipe = LCProcessRecipe.new()
	recipe.recipe_id = data.get("id", "")
	recipe.recipe_name = data.get("name", "")
	recipe.description = data.get("description", "")
	recipe.duration = data.get("duration", 1.0)
	recipe.power_required = data.get("power_required", 0.0)
	recipe.heat_generated = data.get("heat_generated", 0.0)
	
	# Parse inputs
	var inputs = data.get("inputs", [])
	for input_data in inputs:
		recipe.input_resources.append(LCProcessIngredient.from_dict(input_data))
	
	# Parse outputs
	var outputs = data.get("outputs", [])
	for output_data in outputs:
		recipe.output_resources.append(LCProcessProduct.from_dict(output_data))
	
	return recipe
