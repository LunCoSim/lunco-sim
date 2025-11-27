class_name LCResourceContainer
extends RefCounted

## Runtime instance of a resource with amount and state
##
## Represents a quantity of a specific resource type.
## Tracks amount, temperature, pressure, and provides transfer methods.

var definition: LCResourceDefinition
var amount: float = 0.0
var temperature: float = 293.15  # K (20°C)
var pressure: float = 101325.0   # Pa (1 atm)

func _init(res_def: LCResourceDefinition, initial_amount: float = 0.0):
	definition = res_def
	amount = initial_amount

## Get total mass of this resource
func get_mass() -> float:
	if not definition:
		return 0.0
	return amount * definition.density

## Get thermal energy stored in this resource
func get_thermal_energy() -> float:
	if not definition:
		return 0.0
	return amount * definition.specific_heat * temperature

## Add resource amount
func add(delta: float) -> float:
	var old_amount = amount
	amount += delta
	amount = max(0.0, amount)  # Can't be negative
	return amount - old_amount

## Remove resource amount
func remove(delta: float) -> float:
	var removed = min(delta, amount)
	amount -= removed
	return removed

## Transfer resource to another container
func transfer_to(other: LCResourceContainer, delta: float) -> float:
	if not other or not other.definition:
		return 0.0
	
	if other.definition.resource_id != definition.resource_id:
		push_error("Cannot transfer between different resource types")
		return 0.0
	
	var transferred = remove(delta)
	other.add(transferred)
	
	# Mix temperatures (simple average weighted by mass)
	if transferred > 0:
		var total_mass = other.get_mass()
		if total_mass > 0:
			other.temperature = (other.temperature * (total_mass - transferred * definition.density) + temperature * transferred * definition.density) / total_mass
	
	return transferred

## Get fill percentage (requires max capacity from tank)
func get_fill_percentage(max_capacity: float) -> float:
	if max_capacity <= 0:
		return 0.0
	return (amount / max_capacity) * 100.0

## Check if empty
func is_empty() -> bool:
	return amount <= 0.001  # Small epsilon for floating point

## Check if has at least specified amount
func has_amount(required: float) -> bool:
	return amount >= required

## Get resource name
func get_resource_name() -> String:
	if definition:
		return definition.display_name
	return "Unknown"

## Get resource ID
func get_resource_id() -> String:
	if definition:
		return definition.resource_id
	return ""
