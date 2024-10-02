@tool
# Basic living facility that consumes electricity and oxygen per person
class_name LCFacilityLiving
extends LCFacilityBlank

@export var max_occupants: int = 10
@export var current_occupants: int = 0
@export var electricity_consumption_per_person: float = 1.0  # kWh per hour
@export var oxygen_consumption_per_person: float = 0.84  # kg per day

var total_electricity_consumption: float = 0.0
var total_oxygen_consumption: float = 0.0

func _ready():
	super._ready()
	update_consumption()

func update_consumption():
	total_electricity_consumption = current_occupants * electricity_consumption_per_person
	total_oxygen_consumption = current_occupants * oxygen_consumption_per_person

func add_occupant():
	if current_occupants < max_occupants:
		current_occupants += 1
		update_consumption()

func remove_occupant():
	if current_occupants > 0:
		current_occupants -= 1
		update_consumption()

func _process(delta):
	# Calculate consumption per frame
	var electricity_consumed = total_electricity_consumption * delta / 3600  # Convert from per hour to per second
	var oxygen_consumed = total_oxygen_consumption * delta / 86400  # Convert from per day to per second
	
	# Here you would implement the actual consumption of resources
	# For example, you might have a global resource manager:
	# ResourceManager.consume_electricity(electricity_consumed)
	# ResourceManager.consume_oxygen(oxygen_consumed)

func set_max_occupants(new_max: int):
	max_occupants = new_max
	if current_occupants > max_occupants:
		current_occupants = max_occupants
	update_consumption()
