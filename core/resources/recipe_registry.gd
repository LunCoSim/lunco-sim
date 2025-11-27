extends Node

## Global registry for process recipes
##
## Manages all available recipes for resource conversion.
## Recipes can be registered from code, .tres files, or JSON.

var recipes: Dictionary = {}  # recipe_id -> LCProcessRecipe

signal recipe_registered(recipe: LCProcessRecipe)

func _ready():
	print("RecipeRegistry: Initializing")
	_load_builtin_recipes()
	_load_user_recipes()
	print("RecipeRegistry: Loaded ", recipes.size(), " recipes")

## Register a recipe
func register_recipe(recipe: LCProcessRecipe) -> bool:
	if not recipe or recipe.recipe_id.is_empty():
		push_error("RecipeRegistry: Invalid recipe")
		return false
	
	if recipes.has(recipe.recipe_id):
		push_warning("RecipeRegistry: Recipe already registered: " + recipe.recipe_id)
		return false
	
	recipes[recipe.recipe_id] = recipe
	recipe_registered.emit(recipe)
	print("RecipeRegistry: Registered recipe: ", recipe.recipe_name, " (", recipe.recipe_id, ")")
	return true

## Register recipe from dictionary (for JSON loading)
func register_recipe_from_dict(data: Dictionary) -> bool:
	var recipe = LCProcessRecipe.from_dict(data)
	return register_recipe(recipe)

## Get recipe by ID
func get_recipe(recipe_id: String) -> LCProcessRecipe:
	return recipes.get(recipe_id)

## Get all recipes
func get_all_recipes() -> Array[LCProcessRecipe]:
	var result: Array[LCProcessRecipe] = []
	result.assign(recipes.values())
	return result

## Get recipes that produce a specific resource
func get_recipes_for_output(resource_id: String) -> Array[LCProcessRecipe]:
	var result: Array[LCProcessRecipe] = []
	for recipe in recipes.values():
		for product in recipe.output_resources:
			if product.resource_id == resource_id:
				result.append(recipe)
				break
	return result

## Get recipes that consume a specific resource
func get_recipes_for_input(resource_id: String) -> Array[LCProcessRecipe]:
	var result: Array[LCProcessRecipe] = []
	for recipe in recipes.values():
		for ingredient in recipe.input_resources:
			if ingredient.resource_id == resource_id:
				result.append(recipe)
				break
	return result

## Check if recipe exists
func has_recipe(recipe_id: String) -> bool:
	return recipes.has(recipe_id)

# Load built-in recipes from res://core/resources/recipes/
func _load_builtin_recipes():
	var dir = DirAccess.open("res://core/resources/recipes/")
	if not dir:
		print("RecipeRegistry: No built-in recipes directory found")
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	while file_name != "":
		if file_name.ends_with(".tres") or file_name.ends_with(".res"):
			var res = load("res://core/resources/recipes/" + file_name)
			if res is LCProcessRecipe:
				register_recipe(res)
		elif file_name.ends_with(".json"):
			_load_recipe_from_json("res://core/resources/recipes/" + file_name)
		file_name = dir.get_next()

# Load user-defined recipes from user://recipes/
func _load_user_recipes():
	var dir = DirAccess.open("user://recipes/")
	if not dir:
		# Create directory if it doesn't exist
		DirAccess.make_dir_absolute("user://recipes/")
		return
	
	dir.list_dir_begin()
	var file_name = dir.get_next()
	while file_name != "":
		if file_name.ends_with(".json"):
			_load_recipe_from_json("user://recipes/" + file_name)
		file_name = dir.get_next()

func _load_recipe_from_json(path: String):
	var file = FileAccess.open(path, FileAccess.READ)
	if not file:
		return
	
	var json_text = file.get_as_text()
	var json = JSON.parse_string(json_text)
	
	if json and json is Dictionary:
		register_recipe_from_dict(json)
	else:
		push_error("RecipeRegistry: Failed to parse JSON: " + path)
