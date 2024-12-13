class_name ResourceFlow
extends Resource

var resource_type: String
var amount: float
var from_node: String
var to_node: String

func _init(type: String, amt: float, from: String, to: String):
    resource_type = type
    amount = amt
    from_node = from
    to_node = to
