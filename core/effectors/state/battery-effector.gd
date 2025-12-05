class_name LCBatteryEffector
extends LCStateEffector

## Battery state effector for energy storage and power management.
##
## Stores electrical energy and manages charge/discharge cycles.
## Includes state of charge tracking and degradation modeling.

@export_group("Battery Properties")
@export var capacity: float = 1000.0  ## Battery capacity in Watt-hours (Wh)
@export var max_charge_rate: float = 100.0  ## Maximum charge rate in Watts
@export var max_discharge_rate: float = 200.0  ## Maximum discharge rate in Watts
@export var nominal_voltage: float = 28.0  ## Nominal voltage in Volts

@export_group("Performance")
@export var charge_efficiency: float = 0.95  ## Charge efficiency (0.0 to 1.0)
@export var discharge_efficiency: float = 0.98  ## Discharge efficiency (0.0 to 1.0)
@export var self_discharge_rate: float = 0.001  ## Self-discharge rate in Wh/hour
@export var internal_resistance: float = 0.1  ## Internal resistance in Ohms

@export_group("State")
@export var initial_charge: float = 1000.0  ## Initial charge in Wh
@export var min_charge: float = 100.0  ## Minimum safe charge in Wh (10% typical)

@export_group("Degradation")
@export var enable_degradation: bool = false  ## Enable battery degradation
@export var cycle_life: int = 1000  ## Number of full charge/discharge cycles
@export var degradation_per_cycle: float = 0.0001  ## Capacity loss per cycle

# Internal state
var current_charge: float = 1000.0  ## Current charge in Wh
var state_of_charge: float = 1.0  ## SoC as fraction (0.0 to 1.0)
var is_charging: bool = false
var is_discharging: bool = false
var charge_rate: float = 0.0  ## Current charge rate in Watts
var discharge_rate: float = 0.0  ## Current discharge rate in Watts
var total_cycles: float = 0.0  ## Total charge/discharge cycles
var health: float = 1.0  ## Battery health (1.0 = new, 0.0 = dead)

# Tracking
var total_energy_charged: float = 0.0  ## Total energy charged in Wh
var total_energy_discharged: float = 0.0  ## Total energy discharged in Wh
var cycle_depth: float = 0.0  ## Current cycle depth for tracking

# Solver Integration
var solver_graph: LCSolverGraph
var solver_node: LCSolverNode  ## Electrical node representing battery terminal

func _ready():
	super._ready()
	current_charge = initial_charge
	state_of_charge = current_charge / capacity if capacity > 0 else 0.0
	mass = 10.0 + capacity * 0.01  # Rough mass estimate (10kg + 10g per Wh)
	_initialize_telemetry()

## Set the solver graph (called by spacecraft during _ready)
func set_solver_graph(graph: LCSolverGraph):
	solver_graph = graph
	if solver_graph and not solver_node:
		# Create electrical storage node
		solver_node = solver_graph.add_node(nominal_voltage, false, "Electrical")
		solver_node.resource_type = "electrical_power"
		
		# Set capacitance (simplified: treat battery as large capacitor)
		# Energy = 0.5 * C * V^2, but for batteries we use Charge = Capacity_Ah * 3600
		# Capacitance (Farads) = Charge / Voltage
		var capacity_coulombs = (capacity / nominal_voltage) * 3600.0  # Wh / V = Ah, * 3600 = Coulombs
		solver_node.set_capacitance(capacity_coulombs / nominal_voltage)  # C = Q/V
		
		# Initialize flow accumulation (charge)
		solver_node.flow_accumulation = (current_charge / nominal_voltage) * 3600.0
		
		print("Battery: Created solver node with capacitance %.2f F" % solver_node.capacitance)

var telemetry_timer: float = 0.0

func _physics_process(delta):
	# Sync with solver if available
	if solver_node:
		# Read charge from solver (Coulombs -> Wh)
		var charge_coulombs = solver_node.flow_accumulation
		current_charge = (charge_coulombs * nominal_voltage) / 3600.0
		
		# Update voltage from solver
		# For now, use solver potential directly as voltage
		# In a more complex model, we'd have internal resistance
		
	_update_battery_state(delta)
	_update_degradation()

func _process(delta):
	telemetry_timer += delta
	if telemetry_timer >= 0.1:
		telemetry_timer = 0.0
		_update_telemetry()

## Charges the battery with the given power in Watts.
## Returns actual power consumed (may be less if battery is full or rate-limited).
func charge(power_watts: float, delta: float) -> float:
	if power_watts <= 0.0:
		return 0.0
	
	# Limit charge rate
	var actual_power = min(power_watts, max_charge_rate)
	
	# Calculate energy to add (accounting for efficiency)
	var energy_to_add = actual_power * delta / 3600.0 * charge_efficiency  # Convert to Wh
	
	# Check if battery is full
	var available_capacity = capacity * health - current_charge
	if available_capacity <= 0.0:
		charge_rate = 0.0
		is_charging = false
		return 0.0
	
	# Limit by available capacity
	energy_to_add = min(energy_to_add, available_capacity)
	actual_power = energy_to_add * 3600.0 / delta / charge_efficiency
	
	current_charge += energy_to_add
	charge_rate = actual_power
	is_charging = true
	total_energy_charged += energy_to_add
	
	return actual_power

## Discharges the battery with the given power in Watts.
## Returns actual power delivered (may be less if battery is empty or rate-limited).
func discharge(power_watts: float, delta: float) -> float:
	if power_watts <= 0.0:
		return 0.0
	
	# Limit discharge rate
	var actual_power = min(power_watts, max_discharge_rate)
	
	# Calculate energy to remove
	var energy_to_remove = actual_power * delta / 3600.0 / discharge_efficiency  # Convert to Wh
	
	# Check if battery is empty
	var available_energy = current_charge - min_charge
	if available_energy <= 0.0:
		discharge_rate = 0.0
		is_discharging = false
		return 0.0
	
	# Limit by available energy
	energy_to_remove = min(energy_to_remove, available_energy)
	actual_power = energy_to_remove * 3600.0 / delta * discharge_efficiency
	
	current_charge -= energy_to_remove
	discharge_rate = actual_power
	is_discharging = true
	total_energy_discharged += energy_to_remove
	
	return actual_power

## Updates battery state (self-discharge, SoC).
func _update_battery_state(delta: float):
	# Self-discharge
	var self_discharge = self_discharge_rate * delta / 3600.0
	current_charge -= self_discharge
	current_charge = max(0.0, current_charge)
	
	# Update state of charge
	var effective_capacity = capacity * health
	state_of_charge = current_charge / effective_capacity if effective_capacity > 0 else 0.0
	
	# Reset charging/discharging flags if not actively charging/discharging
	if charge_rate < 0.1:
		is_charging = false
		charge_rate = 0.0
	if discharge_rate < 0.1:
		is_discharging = false
		discharge_rate = 0.0

## Updates battery degradation based on cycles.
func _update_degradation():
	if enable_degradation:
		# Track cycle depth (simplified: based on total energy throughput)
		var total_throughput = total_energy_charged + total_energy_discharged
		total_cycles = total_throughput / (capacity * 2.0) if capacity > 0 else 0.0
		
		# Calculate health degradation
		if total_cycles > 0:
			health = 1.0 - (total_cycles * degradation_per_cycle)
			health = max(0.0, health)

## Returns true if battery is full.
func is_full() -> bool:
	return state_of_charge >= 0.99

## Returns true if battery is empty (at minimum charge).
func is_empty() -> bool:
	return current_charge <= min_charge

## Returns true if battery is critically low.
func is_critical() -> bool:
	return state_of_charge < 0.2

## Returns available energy in Wh.
func get_available_energy() -> float:
	return max(0.0, current_charge - min_charge)

## Returns current voltage (simplified model).
func get_voltage() -> float:
	# Simple linear model: voltage drops with SoC
	return nominal_voltage * (0.8 + 0.2 * state_of_charge)

## Returns current power flow (positive = charging, negative = discharging).
func get_power_flow() -> float:
	if is_charging:
		return charge_rate
	elif is_discharging:
		return -discharge_rate
	return 0.0

func _initialize_telemetry():
	Telemetry = {
		"current_charge": current_charge,
		"state_of_charge": state_of_charge,
		"is_charging": is_charging,
		"is_discharging": is_discharging,
		"charge_rate": charge_rate,
		"discharge_rate": discharge_rate,
		"voltage": get_voltage(),
		"health": health,
		"total_cycles": total_cycles,
	}

func _update_telemetry():
	Telemetry["current_charge"] = current_charge
	Telemetry["state_of_charge"] = state_of_charge
	Telemetry["is_charging"] = is_charging
	Telemetry["is_discharging"] = is_discharging
	Telemetry["charge_rate"] = charge_rate
	Telemetry["discharge_rate"] = discharge_rate
	Telemetry["voltage"] = get_voltage()
	Telemetry["health"] = health
	Telemetry["total_cycles"] = total_cycles
