class_name Token
extends RefCounted

var type: String
var value: String
var position: int

func _init(p_type: String, p_value: String, p_position: int):
	type = p_type
	value = p_value
	position = p_position

func _to_string() -> String:
	return "Token(%s, '%s', %d)" % [type, value, position] 