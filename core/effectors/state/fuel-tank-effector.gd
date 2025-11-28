class_name LCFuelTankEffector
extends LCStateEffector

## Fuel tank state effector with mass depletion.
##
## Tracks fuel mass and provides dynamic mass contribution.
## Center of mass shifts as fuel depletes.

@export_group("Fuel Tank Properties")
@export var fuel_capacity: float = 100.0  ## Maximum fuel capacity in kg
@export var fuel_mass: float = 100.0  ## Current fuel mass in kg
@export var tank_dry_mass: float = 10.0  ## Empty tank mass in kg
@export var fuel_density: float = 1000.0  ## Fuel density in kg/m³ (1000 for hydrazine)

@export_group("Tank Geometry")
@export var tank_type: TankType = TankType.SPHERICAL
@export var tank_radius: float = 0.5  ## Tank radius in meters
@export var tank_height: float = 1.0  ## Tank height for cylindrical tanks

enum TankType {
	SPHERICAL,
	CYLINDRICAL,
	CUSTOM
}

# Internal state
var fuel_level: float = 1.0  ## Fuel level as fraction (0.0 to 1.0)

func _ready():
	super._ready()
	mass = tank_dry_mass + fuel_mass
	fuel_level = fuel_mass / fuel_capacity
	_initialize_telemetry()
	
	# Register Parameters
	Parameters["Capacity"] = { "path": "fuel_capacity", "type": "float", "min": 10.0, "max": 2000.0, "step": 10.0 }
	Parameters["Fuel Mass"] = { "path": "fuel_mass", "type": "float", "min": 0.0, "max": 2000.0, "step": 1.0 }
	Parameters["Dry Mass"] = { "path": "tank_dry_mass", "type": "float", "min": 1.0, "max": 500.0, "step": 1.0 }

## Depletes fuel by the given mass in kg.
## Returns actual mass depleted (may be less if tank runs dry).
func deplete_fuel(mass_kg: float) -> float:
	var actual_depletion = min(mass_kg, fuel_mass)
	if actual_depletion > 0:
		fuel_mass -= actual_depletion
		fuel_mass = max(0.0, fuel_mass)
		_update_mass()
	return actual_depletion

## Refills fuel by the given mass in kg.
## Returns actual mass added (may be less if tank is full).
func refill_fuel(mass_kg: float) -> float:
	var actual_refill = min(mass_kg, fuel_capacity - fuel_mass)
	if actual_refill > 0:
		fuel_mass += actual_refill
		fuel_mass = min(fuel_capacity, fuel_mass)
		_update_mass()
	return actual_refill

func _update_mass():
	mass = tank_dry_mass + fuel_mass
	fuel_level = fuel_mass / fuel_capacity if fuel_capacity > 0 else 0.0
	mass_changed.emit()
	_update_telemetry()

## Returns the total mass contribution (tank + fuel).
func get_mass_contribution() -> float:
	return tank_dry_mass + fuel_mass

## Returns the center of mass offset.
## For now, assumes fuel is evenly distributed.
## TODO: Model fuel slosh and center of mass shift in partial tanks.
func get_center_of_mass_offset() -> Vector3:
	# Simple model: COM shifts down as fuel depletes in cylindrical tanks
	if tank_type == TankType.CYLINDRICAL:
		var fuel_fraction = fuel_mass / fuel_capacity if fuel_capacity > 0 else 0.0
		var com_shift = Vector3(0, -tank_height * 0.25 * (1.0 - fuel_fraction), 0)
		return position + com_shift
	else:
		return position

## Returns the current fuel volume in m³.
func get_fuel_volume() -> float:
	return fuel_mass / fuel_density if fuel_density > 0 else 0.0

## Returns true if tank is empty.
func is_empty() -> bool:
	return fuel_mass <= 0.0

## Returns true if tank is full.
func is_full() -> bool:
	return fuel_mass >= fuel_capacity

func _initialize_telemetry():
	Telemetry = {
		"fuel_mass": fuel_mass,
		"fuel_level": fuel_level,
		"fuel_capacity": fuel_capacity,
		"tank_dry_mass": tank_dry_mass,
		"total_mass": get_mass_contribution(),
	}

func _update_telemetry():
	Telemetry["fuel_mass"] = fuel_mass
	Telemetry["fuel_level"] = fuel_level
	Telemetry["total_mass"] = get_mass_contribution()
