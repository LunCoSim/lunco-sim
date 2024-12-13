class_name ResourceNode
extends SimulationNode

var current_amount: float
var max_amount: float

func _init(id: String):
    super._init(id, "resource")
    pass

func remove_resource(amount: float) -> float:
    var available = min(amount, current_amount)
    current_amount -= available
    return available
