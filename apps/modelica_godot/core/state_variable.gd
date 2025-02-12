class_name StateVariable
extends RefCounted

var name: String
var value: float = 0.0
var derivative: float = 0.0
var derivative_of: StateVariable = null
var derivatives: Array[StateVariable] = []
var component: ModelicaComponent

func _init(var_name: String, comp: ModelicaComponent) -> void:
	name = var_name
	component = comp
	derivatives = []

func is_base_variable() -> bool:
	return derivative_of == null

func is_derivative() -> bool:
	return derivative_of != null

func get_order() -> int:
	var order = 0
	var current = self
	while current.derivative_of != null:
		order += 1
		current = current.derivative_of
	return order

func get_base_variable() -> StateVariable:
	var current = self
	while current.derivative_of != null:
		current = current.derivative_of
	return current

func _to_string() -> String:
	return "%s (value=%.3f, der=%.3f)" % [name, value, derivative] 