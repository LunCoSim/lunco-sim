class_name ModelicaComponent
extends Node

var connectors: Dictionary = {}
var parameters: Dictionary = {}
var variables: Dictionary = {}
var equations: Array[String] = []

func add_connector(name: String, type: ModelicaConnector.Type) -> void:
	connectors[name] = ModelicaConnector.new(type)

func add_parameter(name: String, value: float) -> void:
	parameters[name] = value

func add_variable(name: String, initial_value: float = 0.0) -> void:
	variables[name] = initial_value

func add_equation(equation: String) -> void:
	equations.append(equation)

func get_connector(name: String) -> ModelicaConnector:
	return connectors.get(name)

func get_parameter(name: String) -> float:
	return parameters.get(name, 0.0)

func get_variable(name: String) -> float:
	return variables.get(name, 0.0)

func get_equations() -> Array[String]:
	return equations 
