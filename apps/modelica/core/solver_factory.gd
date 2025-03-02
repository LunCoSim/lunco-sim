class_name SolverFactory

# Solver types enum
enum SolverType {
	AUTO,        # Automatically choose best solver
	CAUSAL,      # Explicit causal solver
	RK4,         # 4th-order Runge-Kutta for ODEs
	ACAUSAL      # Acausal equation-based solver
}

# Create a solver for the given equation system
func create_solver(equation_system: Object, solver_type: int = SolverType.AUTO) -> Object:
	if solver_type == SolverType.AUTO:
		return _select_best_solver(equation_system)
	elif solver_type == SolverType.CAUSAL:
		return _create_causal_solver(equation_system)
	elif solver_type == SolverType.RK4:
		return _create_rk4_solver(equation_system)
	elif solver_type == SolverType.ACAUSAL:
		return _create_acausal_solver(equation_system)
	else:
		push_error("Unknown solver type: " + str(solver_type))
		return null

# Analyze the equation system and select the most appropriate solver
func _select_best_solver(equation_system: Object) -> Object:
	var has_state_variables = equation_system.get_state_variables().size() > 0
	var has_differential_equations = false
	
	for eq in equation_system.equations:
		if eq.type == ModelicaEquation.EquationType.DIFFERENTIAL:
			has_differential_equations = true
			break
	
	if has_state_variables and has_differential_equations:
		# System has differential equations, use RK4
		print("Auto-selected RK4 solver for system with differential equations")
		return _create_rk4_solver(equation_system)
	else:
		# Try to use causal solver for pure algebraic systems
		var solver = _create_causal_solver(equation_system)
		if solver.initialized:
			print("Auto-selected causal solver for algebraic system")
			return solver
		else:
			# If causal solver initialization fails, fall back to acausal solver
			push_warning("Causal solver initialization failed, falling back to acausal solver")
			return _create_acausal_solver(equation_system)

# Create and initialize a causal solver
func _create_causal_solver(equation_system: Object) -> Object:
	var solver = CausalSolver.new()
	solver.initialize(equation_system)
	return solver

# Create and initialize an RK4 solver
func _create_rk4_solver(equation_system: Object) -> Object:
	var solver = RK4Solver.new()
	solver.initialize(equation_system)
	return solver

# Create and initialize an acausal solver
func _create_acausal_solver(equation_system: Object) -> Object:
	# This is a placeholder - acausal solver to be implemented later
	push_warning("Acausal solver not yet implemented, using causal solver as fallback")
	return _create_causal_solver(equation_system) 