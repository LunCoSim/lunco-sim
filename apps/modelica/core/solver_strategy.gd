class_name SolverStrategy
extends RefCounted

# The equation system this solver operates on
var equation_system = null

# Initialize the solver with an equation system
func initialize(eq_system) -> bool:
	equation_system = eq_system
	return _initialize_impl()

# Implementation of initialization (to be overridden by subclasses)
func _initialize_impl() -> bool:
	push_error("_initialize_impl not implemented in base SolverStrategy")
	return false

# Take a step in time with the given time step dt
func step(dt: float) -> bool:
	if equation_system == null:
		push_error("Cannot step: equation system is null")
		return false
	
	return _step_impl(dt)

# Implementation of step (to be overridden by subclasses)
func _step_impl(dt: float) -> bool:
	push_error("_step_impl not implemented in base SolverStrategy")
	return false

# Get the current state of the system
func get_state() -> Dictionary:
	if equation_system == null:
		push_error("Cannot get state: equation system is null")
		return {}
	
	return equation_system.get_state()

# Set the state of the system
func set_state(state: Dictionary) -> void:
	if equation_system == null:
		push_error("Cannot set state: equation system is null")
		return
	
	equation_system.set_state(state)

# Reset the system to initial conditions
func reset() -> void:
	if equation_system == null:
		push_error("Cannot reset: equation system is null")
		return
	
	equation_system.reset()
	_initialize_impl() 