class_name LCProcessEffector
extends LCStateEffector

## Base class for resource conversion processes
##
## Processes convert input resources to output resources based on a recipe.
## Can be simple (mass balance) or complex (Modelica-backed).

@export var recipe_id: String = ""  ## Recipe to use for this process
@export var is_active: bool = false  ## Is process currently running
@export var auto_start: bool = false  ## Start automatically when inputs available

var recipe: LCProcessRecipe
var cycle_progress: float = 0.0
var connected_tanks: Dictionary = {}  # resource_id -> LCResourceEffector
var current_efficiency: float = 1.0

# Statistics
var total_cycles_completed: int = 0
var total_runtime: float = 0.0

func _ready():
	super._ready()
	_load_recipe()
	_initialize_telemetry()

func _load_recipe():
	if recipe_id.is_empty():
		push_warning("ProcessEffector: No recipe specified")
		return
	
	recipe = LCRecipeRegistry.get_recipe(recipe_id)
	if recipe:
		print("ProcessEffector: Loaded recipe: ", recipe.recipe_name)
	else:
		push_error("ProcessEffector: Recipe not found: " + recipe_id)

func _physics_process(delta: float):
	if not is_active or not recipe:
		return
	
	# Check if we have enough inputs
	if not _check_inputs():
		if auto_start:
			is_active = false  # Stop if inputs run out
		return
	
	# Progress the cycle
	cycle_progress += delta
	total_runtime += delta
	
	# Complete cycle when duration reached
	if cycle_progress >= recipe.duration:
		_execute_cycle()
		cycle_progress = 0.0
		total_cycles_completed += 1

func _process(delta):
	_update_telemetry()

## Execute one process cycle
func _execute_cycle():
	# Consume inputs
	for ingredient in recipe.input_resources:
		var tank = connected_tanks.get(ingredient.resource_id)
		if tank:
			var consumed = tank.remove_resource(ingredient.amount_per_cycle * current_efficiency)
			if consumed < ingredient.amount_per_cycle * current_efficiency * 0.99:
				push_warning("ProcessEffector: Insufficient input: " + ingredient.resource_id)
	
	# Produce outputs
	for product in recipe.output_resources:
		var tank = connected_tanks.get(product.resource_id)
		if tank:
			var produced = tank.add_resource(product.amount_per_cycle * current_efficiency)
			if produced < product.amount_per_cycle * current_efficiency * 0.99:
				push_warning("ProcessEffector: Output tank full: " + product.resource_id)
	
	# Generate heat (future: thermal system integration)
	if recipe.heat_generated > 0:
		pass  # TODO: Add to thermal system

## Check if all inputs are available
func _check_inputs() -> bool:
	if not recipe:
		return false
	
	for ingredient in recipe.input_resources:
		var tank = connected_tanks.get(ingredient.resource_id)
		if not tank:
			return false
		if tank.get_amount() < ingredient.amount_per_cycle * current_efficiency:
			return false
	
	return true

## Check if all outputs have space
func _check_outputs() -> bool:
	if not recipe:
		return false
	
	for product in recipe.output_resources:
		var tank = connected_tanks.get(product.resource_id)
		if not tank:
			return false
		if tank.is_full():
			return false
	
	return true

## Connect a resource tank to this process
func connect_tank(tank: LCResourceEffector):
	if not tank:
		return
	
	connected_tanks[tank.resource_id] = tank
	print("ProcessEffector: Connected tank for ", tank.get_resource_name())

## Disconnect a tank
func disconnect_tank(resource_id: String):
	connected_tanks.erase(resource_id)

## Start the process
func start_process():
	if not recipe:
		push_error("ProcessEffector: Cannot start - no recipe loaded")
		return
	
	if not _check_inputs():
		push_warning("ProcessEffector: Cannot start - insufficient inputs")
		return
	
	is_active = true
	print("ProcessEffector: Started ", recipe.recipe_name)

## Stop the process
func stop_process():
	is_active = false
	cycle_progress = 0.0
	print("ProcessEffector: Stopped")

## Get process status
func get_status() -> String:
	if not recipe:
		return "No Recipe"
	if is_active:
		return "Running"
	if not _check_inputs():
		return "Insufficient Inputs"
	if not _check_outputs():
		return "Outputs Full"
	return "Ready"

## Get cycle completion percentage
func get_cycle_progress() -> float:
	if not recipe or recipe.duration <= 0:
		return 0.0
	return (cycle_progress / recipe.duration) * 100.0

func _initialize_telemetry():
	Telemetry = {
		"recipe_id": recipe_id,
		"recipe_name": recipe.recipe_name if recipe else "None",
		"is_active": is_active,
		"status": get_status(),
		"cycle_progress": 0.0,
		"efficiency": current_efficiency,
		"total_cycles": total_cycles_completed,
		"total_runtime": total_runtime,
		"power_consumption": recipe.power_required if recipe else 0.0,
	}

func _update_telemetry():
	if not Telemetry:
		return
	
	Telemetry["is_active"] = is_active
	Telemetry["status"] = get_status()
	Telemetry["cycle_progress"] = get_cycle_progress()
	Telemetry["efficiency"] = current_efficiency
	Telemetry["total_cycles"] = total_cycles_completed
	Telemetry["total_runtime"] = total_runtime

# Command interface
func cmd_start():
	start_process()

func cmd_stop():
	stop_process()

func cmd_toggle():
	if is_active:
		stop_process()
	else:
		start_process()
