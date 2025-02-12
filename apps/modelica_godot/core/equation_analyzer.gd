class_name EquationAnalyzer
extends RefCounted

var equations: Array[Dictionary]
var variable_dependencies: Dictionary
var equation_dependencies: Dictionary
var equation_computes: Dictionary
var sorted_equations: Array[int]
var differential_equations: Array[int]
var algebraic_equations: Array[int]

func _init(p_equations: Array[Dictionary]):
	equations = p_equations
	variable_dependencies = {}
	equation_dependencies = {}
	equation_computes = {}
	sorted_equations = []
	differential_equations = []
	algebraic_equations = []

func analyze() -> void:
	_classify_equations()
	_build_dependencies()
	_sort_equations()
	print("Equation Analysis:")
	print("- Differential equations: ", differential_equations)
	print("- Algebraic equations: ", algebraic_equations)
	print("- Dependencies: ", equation_dependencies)
	print("- Computes: ", equation_computes)
	print("- Final sort order: ", sorted_equations)

func _classify_equations() -> void:
	for i in range(equations.size()):
		var eq = equations[i]
		if eq.is_differential:
			differential_equations.append(i)
			var der_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			equation_computes[i] = der_var
		else:
			algebraic_equations.append(i)
			equation_computes[i] = eq.left

func _build_dependencies() -> void:
	for i in range(equations.size()):
		var eq = equations[i]
		var eq_deps: Array[String] = []
		
		if eq.left_ast:
			eq_deps.append_array(eq.left_ast.get_dependencies())
		if eq.right_ast:
			eq_deps.append_array(eq.right_ast.get_dependencies())
		
		if eq.is_differential:
			var state_var = _extract_der_variable(eq.left if eq.left.begins_with("der(") else eq.right)
			if not eq_deps.has(state_var):
				eq_deps.append(state_var)
		
		equation_dependencies[i] = eq_deps
		
		for var_name in eq_deps:
			if not variable_dependencies.has(var_name):
				variable_dependencies[var_name] = []
			for other_var in eq_deps:
				if other_var != var_name and not variable_dependencies[var_name].has(other_var):
					variable_dependencies[var_name].append(other_var)

func _sort_equations() -> void:
	var visited = {}
	var temp_mark = {}
	sorted_equations = []
	
	for eq_idx in algebraic_equations:
		if not _depends_on_derivative(eq_idx) and not visited.has(eq_idx):
			_visit_node(eq_idx, visited, temp_mark)
	
	for eq_idx in differential_equations:
		if not visited.has(eq_idx):
			_visit_node(eq_idx, visited, temp_mark)
	
	for eq_idx in algebraic_equations:
		if not visited.has(eq_idx):
			_visit_node(eq_idx, visited, temp_mark)

func _depends_on_derivative(eq_idx: int) -> bool:
	var deps = equation_dependencies[eq_idx]
	for dep in deps:
		for diff_eq_idx in differential_equations:
			if equation_computes[diff_eq_idx] == dep:
				return true
	return false

func _visit_node(eq_idx: int, visited: Dictionary, temp_mark: Dictionary) -> void:
	if temp_mark.has(eq_idx):
		push_warning("Cyclic dependency detected in equations")
		return
	if visited.has(eq_idx):
		return
	
	temp_mark[eq_idx] = true
	
	var eq_deps = equation_dependencies[eq_idx]
	for dep_var in eq_deps:
		for i in range(equations.size()):
			if equation_computes.get(i, "") == dep_var:
				_visit_node(i, visited, temp_mark)
	
	temp_mark.erase(eq_idx)
	visited[eq_idx] = true
	sorted_equations.push_front(eq_idx)

func _extract_der_variable(expr: String) -> String:
	var start_idx = expr.find("der(") + 4
	var end_idx = expr.find(")", start_idx)
	if start_idx >= 4 and end_idx > start_idx:
		return expr.substr(start_idx, end_idx - start_idx)
	return "" 