class_name LCProcessProduct
extends Resource

@export var resource_id: String = ""
@export var amount_per_cycle: float = 0.0

func _init(res_id: String = "", amt: float = 0.0):
	resource_id = res_id
	amount_per_cycle = amt

func to_dict() -> Dictionary:
	return {"resource_id": resource_id, "amount": amount_per_cycle}

static func from_dict(data: Dictionary) -> LCProcessProduct:
	return LCProcessProduct.new(data.get("resource_id", ""), data.get("amount", 0.0))
